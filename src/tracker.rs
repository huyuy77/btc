//! Tracker-related data structures and helper functions.

use std::net::{IpAddr, SocketAddr};

use anyhow::Result;
use bt_bencode::ByteString;
use percent_encoding::{NON_ALPHANUMERIC, percent_encode};
use reqwest::{Client, Method, Proxy};
use serde_derive::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use url::Url;

use crate::{
    cache::TorrentCache,
    utils::{as_array_ref, random_client_ua, random_key, random_peer_id, random_port},
};

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AnnounceResponse {
    #[serde(rename = "failure reason")]
    pub(crate) failure_reason: Option<String>,
    #[serde(rename = "warning message")]
    pub(crate) warning_message: Option<String>,
    pub(crate) interval: Option<u64>,
    #[serde(rename = "min interval")]
    pub(crate) min_interval: Option<u64>,
    #[serde(rename = "tracker id")]
    pub(crate) tracker_id: Option<String>,
    #[serde(rename = "complete")]
    pub(crate) seeders: Option<u64>,
    #[serde(rename = "incomplete")]
    pub(crate) leechers: Option<u64>,
    pub(crate) peers: Option<ByteString>,
    /// Peers with IPv6 addresses.
    pub(crate) peers6: Option<ByteString>,
}

impl From<TorrentCache> for AnnounceResponse {
    fn from(value: TorrentCache) -> Self {
        let mut peers = Vec::new();
        let mut peers6 = Vec::new();
        for peer in value.peers_addr.keys() {
            if peer.is_ipv4() {
                peers.extend(serialize_peer_binary(peer));
            } else if peer.is_ipv6() {
                peers6.extend(serialize_peer_binary(peer));
            }
        }

        Self {
            failure_reason: None,
            warning_message: None,
            interval: Some(30),
            min_interval: Some(30),
            tracker_id: None,
            seeders: Some(0),
            leechers: Some(value.peers_addr.len() as u64),
            peers: Some(peers.into()),
            peers6: Some(peers6.into()),
        }
    }
}

pub(crate) fn deserialize_peers_binary(value: &[u8]) -> Vec<SocketAddr> {
    debug_assert_eq!(value.len() % 6, 0);
    value
        .chunks(6)
        .map(|x| unsafe {
            (
                as_array_ref::<4>(&x[..4]).to_owned(),
                u16::from_be_bytes([x[4], x[5]]),
            )
                .into()
        })
        .collect()
}

pub(crate) fn serialize_peer_binary(value: &SocketAddr) -> Vec<u8> {
    match value.ip() {
        IpAddr::V4(addr) => addr
            .as_octets()
            .into_iter()
            .chain(value.port().to_be_bytes().iter())
            .copied()
            .collect(),
        IpAddr::V6(addr) => addr
            .as_octets()
            .into_iter()
            .chain(value.port().to_be_bytes().iter())
            .copied()
            .collect(),
    }
}

pub(crate) fn deserialize_peers6_binary(value: &[u8]) -> Vec<SocketAddr> {
    debug_assert_eq!(value.len() % 18, 0);
    value
        .chunks(18)
        .map(|x| unsafe {
            (
                as_array_ref::<16>(&x[..16]).to_owned(),
                u16::from_be_bytes([x[16], x[17]]),
            )
                .into()
        })
        .collect()
}

/// Announce to the origin tracker. A fixed fake qBittorrent client fingerprint generated from the
/// tracker URL is used as a disguise. To construct a realistic request, the torrent size must be
/// known at this moment.
pub(crate) async fn announce(
    tracker_url: &str,
    info_hash: &[u8],
    size: u64,
) -> Result<AnnounceResponse> {
    let url = Url::parse(tracker_url)?;
    let info_hash_encoded = percent_encode(info_hash, NON_ALPHANUMERIC).to_string();
    // We have to manually concatenate the URL here, because `reqwest` and `url` crate always
    // percent-encode the url components from `String`. Since `info_hash`'es are raw bytes instead
    // of UTF-8 encoded printable strings, the built-in conversion from `reqwest` is LOSSY.
    let url = Url::parse(&if url.query().is_some() {
        format!("{tracker_url}&info_hash={info_hash_encoded}")
    } else if url.path() != "/" || tracker_url.ends_with("/") {
        format!("{tracker_url}?info_hash={info_hash_encoded}")
    } else {
        format!("{tracker_url}/?info_hash={info_hash_encoded}")
    })?;

    let http_client = Client::builder()
        .user_agent(&random_client_ua(tracker_url))
        .gzip(true);

    let http_client = if let Ok(proxy_url) = std::env::var("PROXY") {
        http_client.proxy(Proxy::all(&proxy_url)?)
    } else {
        http_client
    }
    .build()?;

    let req = http_client
        .request(Method::GET, url)
        .query(&[
            ("peer_id", random_peer_id(tracker_url).as_str()),
            ("port", random_port(tracker_url).to_string().as_str()),
            ("uploaded", "0"),
            ("downloaded", "0"),
            ("left", &size.to_string()),
            ("corrupt", "0"),
            ("key", &random_key(tracker_url)),
            ("event", "started"),
            ("numwant", "200"),
            ("compact", "1"),
            ("no_peer_id", "1"),
            ("supportcrypto", "1"),
            ("redundant", "0"),
        ])
        .header(reqwest::header::CONNECTION, "close")
        .build()?;
    eprintln!("{:#?}", req);

    let response = http_client.execute(req).await?;
    eprintln!("{:#?}", response);
    let response_bytes = response.bytes().await?;

    bt_bencode::from_slice(&response_bytes).map_err(|e| anyhow::anyhow!(e))
}
