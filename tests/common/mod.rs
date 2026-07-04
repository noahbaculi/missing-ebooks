//! Shared helpers for the SSE integration tests. Cargo skips files under
//! `tests/common/` for test discovery, so each test crate declares
//! `mod common;` and uses the items here directly.
//!
//! Each test compiles `mod common` independently. A helper used by only some
//! tests still gets imported by every `mod common`, so unused-warning gates
//! apply per test crate. `#[allow(dead_code)]` here keeps the lint clean
//! across every test target that pulls the module in.

#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use axum::body::Body;
use futures_util::StreamExt;
use http_body_util::{BodyExt, BodyStream};

/// Create an empty file at `path`, creating its parents first. Mirror of
/// `crate::scenarios::touch` for integration tests that can't reach the
/// in-crate helper.
pub fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"").unwrap();
}

/// Drain a body into a single String for substring assertions.
pub async fn body_to_string(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Parse one complete SSE event from a buffer. Returns `(event_name, data,
/// rest)` or `None` when the buffer does not hold a complete event yet (events
/// are separated by a blank line. Each event has one or more `event:` /
/// `data:` lines). Multiple `data:` lines join with `\n` per the SSE spec.
pub fn parse_event(buf: &str) -> Option<((String, String), &str)> {
    let end = buf.find("\n\n")?;
    let (event, rest) = buf.split_at(end);
    let mut name = String::new();
    let mut data = String::new();
    for line in event.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.trim_start());
        }
    }
    Some(((name, data), &rest[2..]))
}

/// Read frames from an SSE stream until one complete event is parsed or
/// `deadline_after` elapses. Accumulates partial reads into a single buffer.
pub async fn next_event(
    stream: &mut BodyStream<Body>,
    deadline_after: Duration,
) -> Option<(String, String)> {
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + deadline_after;
    loop {
        if let Some(((name, data), _rest)) = parse_event(&buf) {
            return Some((name, data));
        }
        let frame = tokio::time::timeout_at(deadline, stream.next())
            .await
            .ok()??;
        let frame = frame.ok()?;
        if let Some(bytes) = frame.data_ref() {
            buf.push_str(std::str::from_utf8(bytes).ok()?);
        }
    }
}
