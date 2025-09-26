//! https://github.com/lyc8503/PTHackPoC/blob/79dbeba76b24a445eedddb4fcdba7ef06305cb6f/util/util.go#L15

use anyhow::Result;
use rand::{RngCore as _, SeedableRng as _, rngs::StdRng};
use sha2::Digest as _;
use url::Url;

use crate::bytes_bencode::BencodeObject;

const QB_VERSIONS: [&str; 8] = [
    "-qB5120-", "-qB5110-", "-qB5100-", "-qB5050-", "-qB5040-", "-qB5030-", "-qB5020-", "-qB5010-",
];
const QB_VERSION_UAS: [&str; 8] = [
    "qBittorrent/5.1.2",
    "qBittorrent/5.1.1",
    "qBittorrent/5.1.0",
    "qBittorrent/5.0.5",
    "qBittorrent/5.0.4",
    "qBittorrent/5.0.3",
    "qBittorrent/5.0.2",
    "qBittorrent/5.0.1",
];
const PEER_ID_CHARS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const KEY_CHARS: &str = "0123456789ABCDEF";

fn get_rng(value: &str) -> StdRng {
    unsafe {
        StdRng::seed_from_u64(u64::from_be_bytes(
            sha2::Sha256::digest(value)[0..8]
                .try_into()
                .unwrap_unchecked(),
        ))
    }
}

pub(crate) fn random_client_ua(value: &str) -> String {
    let mut rng = get_rng(value);
    let client_ua_choice = rng.next_u64() as usize % QB_VERSION_UAS.len();
    let client_version = QB_VERSION_UAS[client_ua_choice];
    client_version.to_string()
}

pub(crate) fn random_peer_id(value: &str) -> String {
    let mut rng = get_rng(value);
    let client_version_choice = rng.next_u64() as usize % QB_VERSIONS.len();
    let client_version = QB_VERSIONS[client_version_choice];
    let mut peer_id = client_version.to_string();
    for _ in 0..12 {
        let peer_id_char_choice = rng.next_u64() as usize % PEER_ID_CHARS.len();
        peer_id += &PEER_ID_CHARS[peer_id_char_choice..peer_id_char_choice + 1];
    }
    peer_id
}

pub(crate) fn random_port(value: &str) -> u16 {
    let mut rng = get_rng(value);
    1024 + (rng.next_u64() % (65536 - 1024)) as u16
}

pub(crate) fn random_key(value: &str) -> String {
    let mut rng = get_rng(value);
    let mut key = String::with_capacity(8);
    for _ in 0..8 {
        let key_char_choice = rng.next_u64() as usize % KEY_CHARS.len();
        key += &KEY_CHARS[key_char_choice..key_char_choice + 1];
    }
    key
}

pub(crate) unsafe fn as_array_ref<'a, const N: usize>(slice: &'a [u8]) -> &'a [u8; N] {
    debug_assert!(slice.len() >= N);
    unsafe { &*(slice.as_ptr() as *const [u8; N]) }
}

#[inline]
fn process_tracker_url(value: &str) -> Result<String> {
    const TRACKER_BASE_URL: &str = "https://tracker.submy.org";

    let mut url = Url::parse(TRACKER_BASE_URL)?;
    url.query_pairs_mut()
        .append_pair("tracker_url", value)
        .append_pair("ttl", "28800");
    Ok(url.to_string())
}

pub(crate) fn replace_trackers_in_torrent(torrent: &mut BencodeObject) -> Result<Box<[Box<[u8]>]>> {
    const ANNOUNCE_LITERAL: &[u8] = "announce".as_bytes();
    const ANNOUNCE_LIST_LITERAL: &[u8] = "announce-list".as_bytes();

    let mut result = Vec::new();

    if let BencodeObject::List(obj) = torrent {
        if let BencodeObject::Dictionary(obj) = obj.first_mut().unwrap() {
            for (k, v) in obj {
                if let BencodeObject::Bytes(len, obj) = k
                    && *len != 0
                {
                    if obj.as_ref().unwrap() == ANNOUNCE_LITERAL
                        && let BencodeObject::Bytes(len, obj) = v
                        && *len != 0
                    {
                        let obj = obj.as_mut().unwrap();
                        let new_url =
                            process_tracker_url(&String::from_utf8(std::mem::take(obj))?)?
                                .into_bytes();
                        *len = new_url.len();
                        result.push(std::mem::replace(obj, new_url).into_boxed_slice());
                    } else if obj.as_ref().unwrap() == ANNOUNCE_LIST_LITERAL
                        && let BencodeObject::List(obj) = v
                    {
                        for obj in obj {
                            if let BencodeObject::List(obj) = obj
                                && let BencodeObject::Bytes(len, obj) = obj.first_mut().unwrap()
                                && *len != 0
                            {
                                let obj = obj.as_mut().unwrap();
                                let new_url =
                                    process_tracker_url(&String::from_utf8(std::mem::take(obj))?)?
                                        .into_bytes();
                                *len = new_url.len();
                                result.push(std::mem::replace(obj, new_url).into_boxed_slice());
                            }
                        }
                    }
                }
            }
        }
    }

    // no need to sort, since "announce" and "announce-list" are adjacent to each other
    result.dedup();
    Ok(result.into_boxed_slice())
}
