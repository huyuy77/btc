#![feature(bstr, btree_extract_if, ip_as_octets)]

mod cache;
mod tracker;
mod utils;

use std::{convert::Infallible, time::Duration};

use anyhow::Result;
use percent_encoding::percent_decode_str;
use serde_derive::{Deserialize, Serialize};
use warp::{Filter, http::StatusCode};

use crate::{cache::fetch_cache, tracker::AnnounceResponse};

macro_rules! unwrap_option_or_error {
    ($value: expr) => {{
        let value = $value;
        if value.is_none() {
            return Ok(warp::http::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(warp::hyper::body::Bytes::from(format!(
                    "Missing parameter: {}",
                    stringify!($value)
                )))
                .unwrap());
        }
        unsafe { value.unwrap_unchecked() }
    }};
}

macro_rules! unwrap_result_or_error {
    ($value: expr) => {{
        let value = $value;
        if let Err(error) = value {
            eprintln!("Error: {}", error);
            return Ok(warp::http::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(warp::hyper::body::Bytes::from(format!(
                    "Server error: {}",
                    error
                )))
                .unwrap());
        }
        unsafe { value.unwrap_unchecked() }
    }};
}

#[derive(Serialize, Deserialize, Debug)]
struct AnnounceQuery {
    tracker_url: String,
    ttl: u64,
    downloaded: u64,
    left: u64,
    event: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let announce = warp::get()
        .and(warp::path("announce"))
        .and(warp::query::raw())
        .and(warp::query::<AnnounceQuery>())
        .and_then(move |p: String, q: AnnounceQuery| async move {
            let start = unwrap_option_or_error!(p.find("info_hash="));
            let end = start + 10 + (p[start + 10..].find("&").unwrap_or(p.len() - (start + 10)));
            let info_hash = percent_decode_str(&p[start + 10..end]).collect::<Box<[u8]>>();

            // TODO: re-announce when nothing was downloaded.

            let cache = unwrap_result_or_error!(
                fetch_cache(
                    q.tracker_url,
                    &info_hash,
                    if q.downloaded == 0 {
                        Some(q.left)
                    } else {
                        None
                    },
                    Duration::from_secs(q.ttl)
                )
                .await
            );
            let response: AnnounceResponse = cache.into();

            let bytes = unwrap_result_or_error!(bt_bencode::to_vec(&response));
            let bytes = warp::hyper::body::Bytes::from(bytes);

            Result::<_, Infallible>::Ok(
                warp::http::Response::builder()
                    .status(StatusCode::OK)
                    .body(bytes)
                    .unwrap(),
            )
        });

    let index = warp::get()
        .and(warp::path::end())
        .and(warp::fs::file("www/static/index.html"));

    warp::serve(index.or(announce))
        .run(([0, 0, 0, 0], 3000))
        .await;

    Ok(())
}
