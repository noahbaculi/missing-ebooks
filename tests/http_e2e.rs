//! End-to-end HTTP integration tests. Boots the production `web::router`
//! against a mutable copy of the curated fixture and drives it in-process
//! with `tower::ServiceExt::oneshot`. Coarse wiring-regression net; the
//! tree/scanner contracts are covered elsewhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use missing_ebooks::config::Config;
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::state::AppState;
use missing_ebooks::web;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/curated/Audiobooks")
}

/// Recursively copy `src` into `dst`. `dst` must already exist.
fn copy_dir(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Boot the production router against a fresh tempdir copy of the curated
/// fixture. `extra_roots` are appended after the copied root; use for the
/// errored-root case.
fn boot(extra_roots: Vec<PathBuf>) -> (Router, TempDir) {
    let tmp = TempDir::new().unwrap();
    let audiobooks = tmp.path().join("Audiobooks");
    std::fs::create_dir_all(&audiobooks).unwrap();
    copy_dir(&fixture_root(), &audiobooks);

    let mut roots = vec![audiobooks];
    roots.extend(extra_roots);

    let cfg = Config {
        library_roots: roots,
        ttl_seconds: 60,
        exclude_globs: vec!["**/*(abridged)*".to_string()],
        ..Default::default()
    };
    let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
    let state = Arc::new(AppState::new(cfg, settings));
    (web::router(state), tmp)
}

async fn body_string(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[tokio::test]
async fn boot_smoke_serves_index() {
    let (app, _tmp) = boot(vec![]);
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(!body.is_empty(), "index body must not be empty");
}

#[tokio::test]
async fn index_default_gaps_view_lists_flagged_folders() {
    let (app, _tmp) = boot(vec![]);
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.contains("Elder Race"),
        "default gaps view must list a known-flagged folder"
    );
}

#[tokio::test]
async fn index_all_view_includes_covered_folders() {
    let (app, _tmp) = boot(vec![]);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/?view=all")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.contains("Cage of Souls"),
        "all view must include a known-covered folder"
    );
}

#[tokio::test]
async fn mark_writes_marker_and_reflects_in_response() {
    let (app, tmp) = boot(vec![]);
    let rel = "Adrian Tchaikovsky/Elder Race";
    let body = format!("root=0&rel={}&kind=no_ebook&view=all", urlencode(rel));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mark")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let marker_path = tmp.path().join("Audiobooks").join(rel).join(".no_ebook");
    assert!(
        marker_path.exists(),
        "marker file must be written on disk at {marker_path:?}"
    );

    let text = body_string(response).await;
    assert!(
        text.contains("covered"),
        "response HTML must reflect the newly covered state"
    );
}

#[tokio::test]
async fn unmark_removes_marker() {
    let (app, tmp) = boot(vec![]);
    let rel = "Adrian Tchaikovsky/Elder Race";
    let form = format!("root=0&rel={}&kind=no_ebook&view=gaps", urlencode(rel));

    let mark_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mark")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mark_resp.status(), StatusCode::OK);
    let marker = tmp.path().join("Audiobooks").join(rel).join(".no_ebook");
    assert!(marker.exists(), "precondition: marker must exist");

    let unmark_resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/unmark")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unmark_resp.status(), StatusCode::OK);
    assert!(!marker.exists(), "marker must be removed after /unmark");
}

#[tokio::test]
async fn rescan_returns_ok() {
    let (app, _tmp) = boot(vec![]);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/rescan")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(!body.is_empty(), "rescan response body must not be empty");
}

#[tokio::test]
async fn refresh_returns_swap_payload() {
    let (app, _tmp) = boot(vec![]);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/refresh?view=gaps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(!body.is_empty(), "refresh body must not be empty");
    assert!(
        body.contains("Elder Race"),
        "refresh gaps payload must contain a known-flagged folder"
    );
}

#[tokio::test]
async fn errored_root_renders_banner() {
    let bogus = std::path::PathBuf::from("/nonexistent/missing-ebooks-e2e-does-not-exist");
    let (app, _tmp) = boot(vec![bogus]);
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(
        body.contains("Could not scan this root:"),
        "errored root must render the error banner"
    );
}
