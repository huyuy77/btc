//! Peer list cache backed by cache directory.

use std::{
    collections::{BTreeSet, HashMap},
    mem::take,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::{Duration, SystemTime},
};

use anyhow::Result;
use percent_encoding::{NON_ALPHANUMERIC, percent_encode, utf8_percent_encode};
use serde_derive::{Deserialize, Serialize};
use tokio::{
    fs::create_dir_all,
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    sync::{Mutex, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, Semaphore},
    time::timeout,
};
use url::Url;

use crate::tracker;

/// In-memory solution to concurrent race. The key of the hash map is percent-encoded `info_hash`,
/// and the value consists of a lock and a reference count. When the rc is zeroed, the entry is
/// removed from the map to save memory. Uses read-write locks for better performance.
static CACHE_LOCKS: LazyLock<Mutex<HashMap<String, (Arc<RwLock<()>>, usize)>>> =
    LazyLock::new(Default::default);
/// Since the public instance uses a rotated IP pool which poses a limit on concurrently opened
/// connections, we use a semaphore to control connections to origin trackers.
static TRACKER_CONNECTIONS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(10));

#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Clone, Copy)]
pub(crate) struct Peer {
    pub(crate) expire: SystemTime,
    pub(crate) addr: SocketAddr,
}

/// This struct has two copies of peer list: [`TorrentCache::peers_time`] is for removing overdue
/// peers, and [`TorrentCache::peers_addr`] is for detecting duplicates in the cache.
#[derive(Serialize, Deserialize, Default, Clone)]
pub(crate) struct TorrentCache {
    pub(crate) size: u64,
    pub(crate) trackers: HashMap<String, SystemTime>,
    pub(crate) peers_time: BTreeSet<Peer>,
    pub(crate) peers_addr: HashMap<SocketAddr, SystemTime>,
}

fn get_cache_root_dir() -> PathBuf {
    let cache_root_dir = std::env::var("CACHE_ROOT")
        .map(|x| Path::new(&x).to_owned())
        .unwrap_or(
            std::env::var("XDG_CACHE_HOME")
                .map(|x| Path::new(&x).to_owned())
                .unwrap_or(Path::new(&std::env::var("HOME").unwrap()).join(".cache")),
        );
    cache_root_dir.join("btc")
}

async fn read_cache(info_hash: &str) -> Result<Option<TorrentCache>> {
    let cache_root_dir = get_cache_root_dir();
    let cache_path = cache_root_dir.join(info_hash);

    create_dir_all(cache_root_dir).await?;
    if !tokio::fs::try_exists(&cache_path).await? {
        return Ok(None);
    }

    let mut buf = String::new();
    tokio::fs::File::open(&cache_path)
        .await?
        .read_to_string(&mut buf)
        .await?;
    Ok(Some(serde_json::from_str(&buf)?))
}

async fn write_cache(info_hash: &str, value: &TorrentCache) -> Result<()> {
    let cache_root_dir = get_cache_root_dir();
    let cache_path = cache_root_dir.join(info_hash);

    create_dir_all(cache_root_dir).await?;

    let buf = serde_json::to_string(value)?;
    tokio::fs::File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(cache_path)
        .await?
        .write_all(buf.as_bytes())
        .await?;

    Ok(())
}

/// A wrapper for the cache lock guard, with manually implemented async-drop method to manipulate
/// the outer lock and the rc. You should always use [`CacheLockReadGuard::drop`] instead of
/// [`Drop::drop`].
struct CacheLockReadGuard {
    name: String,
    _inner: OwnedRwLockReadGuard<()>,
}

impl CacheLockReadGuard {
    async fn new(info_hash: &str) -> Self {
        let mut cache_locks = CACHE_LOCKS.lock().await;
        let entry = cache_locks.entry(info_hash.to_string()).or_default();
        entry.1 += 1;
        let entry_lock = entry.0.clone();
        if let Ok(guard) = entry_lock.clone().try_read_owned() {
            return Self {
                name: info_hash.to_string(),
                _inner: guard,
            };
        }

        drop(cache_locks);
        let guard = entry_lock.read_owned().await;

        Self {
            name: info_hash.to_string(),
            _inner: guard,
        }
    }

    async fn drop(mut self) {
        let mut cache_locks = CACHE_LOCKS.lock().await;
        let entry = unsafe { cache_locks.get_mut(&self.name).unwrap_unchecked() };
        let name = take(&mut self.name);
        drop(self);
        entry.1 -= 1;
        if entry.1 == 0 {
            cache_locks.remove(&name);
        }
    }
}

/// A wrapper for the cache lock guard, with manually implemented async-drop method to manipulate
/// the outer lock and the rc. You should always use [`CacheLockWriteGuard::drop`] instead of
/// [`Drop::drop`].
struct CacheLockWriteGuard {
    name: String,
    _inner: OwnedRwLockWriteGuard<()>,
}

impl CacheLockWriteGuard {
    async fn new(info_hash: &str) -> Self {
        let mut cache_locks = CACHE_LOCKS.lock().await;
        let entry = cache_locks.entry(info_hash.to_string()).or_default();
        entry.1 += 1;
        let entry_lock = entry.0.clone();
        if let Ok(guard) = entry_lock.clone().try_write_owned() {
            return Self {
                name: info_hash.to_string(),
                _inner: guard,
            };
        }

        drop(cache_locks);
        let guard = entry_lock.write_owned().await;

        Self {
            name: info_hash.to_string(),
            _inner: guard,
        }
    }

    async fn drop(mut self) {
        let mut cache_locks = CACHE_LOCKS.lock().await;
        let entry = unsafe { cache_locks.get_mut(&self.name).unwrap_unchecked() };
        let name = take(&mut self.name);
        drop(self);
        entry.1 -= 1;
        if entry.1 == 0 {
            cache_locks.remove(&name);
        }
    }
}

/// Clear overdue peers and fetch peer list from origin if needed.
pub(crate) async fn fetch_cache(
    tracker_url: String,
    info_hash: &[u8],
    size: Option<u64>,
    ttl: Duration,
) -> Result<TorrentCache> {
    let mut torrent_size = size;

    let tracker_url_base_encoded = utf8_percent_encode(
        Url::parse(&tracker_url)?.host_str().unwrap(),
        NON_ALPHANUMERIC,
    )
    .to_string();
    let info_hash_encoded = percent_encode(info_hash, NON_ALPHANUMERIC).to_string();

    // If the cache is valid, simply return it.
    let read_lock = CacheLockReadGuard::new(&info_hash_encoded).await;
    let curr_cache = read_cache(&info_hash_encoded).await?;
    read_lock.drop().await;
    if let Some(curr_cache) = curr_cache {
        if let Some(&old_expiration) = curr_cache.trackers.get(&tracker_url_base_encoded)
            && SystemTime::now() < old_expiration
        {
            return Ok(curr_cache);
        }
    };

    // If the cache is invalid but flushed by another task, then also return it.
    // Here since we grab the write lock, there is no need to invoke further validation.
    let write_lock = CacheLockWriteGuard::new(&info_hash_encoded).await;
    let curr_cache = read_cache(&info_hash_encoded).await?;
    if let Some(ref curr_cache) = curr_cache {
        if let Some(&old_expiration) = curr_cache.trackers.get(&tracker_url_base_encoded)
            && SystemTime::now() < old_expiration
        {
            write_lock.drop().await;
            return Ok(curr_cache.clone());
        } else if torrent_size.is_none() {
            torrent_size = Some(curr_cache.size);
        }
    };

    debug_assert!(torrent_size.is_some());
    let torrent_size = torrent_size.unwrap();
    let permit = timeout(Duration::from_secs(30), TRACKER_CONNECTIONS.acquire()).await??;
    let tracker_response = timeout(
        Duration::from_secs(20),
        tracker::announce(&tracker_url, &info_hash, torrent_size),
    )
    .await??;
    drop(permit);

    let mut curr_cache = curr_cache.unwrap_or_default();
    curr_cache.size = torrent_size;
    // Respect the minimum announce interval from origin by keeping the cache valid for at least
    // that long.
    let ttl = if let Some(min_interval) = tracker_response.min_interval {
        Duration::from_secs(min_interval).max(ttl)
    } else {
        ttl
    };
    curr_cache
        .trackers
        .insert(tracker_url_base_encoded.clone(), SystemTime::now() + ttl);
    curr_cache
        .peers_time
        .extract_if(.., |&peer| peer.expire < SystemTime::now())
        .for_each(|peer| {
            curr_cache.peers_addr.remove(&peer.addr);
        });

    let mut new_peers = Vec::new();
    if let Some(peers) = tracker_response.peers {
        new_peers.extend(tracker::deserialize_peers_binary(&peers));
    }
    if let Some(peers6) = tracker_response.peers6 {
        new_peers.extend(tracker::deserialize_peers6_binary(&peers6));
    }
    let expiration = SystemTime::now() + ttl;

    for addr in new_peers {
        if let Some(entry_expiration) = curr_cache.peers_addr.get_mut(&addr) {
            curr_cache.peers_time.remove(&Peer {
                expire: *entry_expiration,
                addr,
            });
            *entry_expiration = expiration;
        } else {
            curr_cache.peers_addr.insert(addr, expiration);
        }
        curr_cache.peers_time.insert(Peer {
            expire: expiration,
            addr,
        });
    }
    write_cache(&info_hash_encoded, &curr_cache).await?;

    write_lock.drop().await;

    Ok(curr_cache)
}
