//! A change that only affects the show-all view (adding a covered audiobook)
//! pushes a `section` event to the show-all subscriber and nothing to the
//! gaps-only subscriber. The per-(mode, root) hash in `compute_pushes` is what
//! makes this work.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyStream;
use missing_ebooks::config::Config;
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::state::AppState;
use missing_ebooks::web::router;
use tower::ServiceExt;

use common::next_event;

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"").unwrap();
}

#[tokio::test]
async fn show_all_only_change_skips_gaps_subscriber() {
    let dir = tempfile::tempdir().unwrap();
    // Seed: one covered audiobook. Gaps view is Clean; show-all renders the
    // covered tree.
    touch(&dir.path().join("Author/Book1/01.mp3"));
    touch(&dir.path().join("Author/Book1/Book1.epub"));

    let config = Config {
        library_roots: vec![dir.path().to_path_buf()],
        ttl_seconds: 600,
        autosync_interval_seconds: 1,
        ..Config::default()
    };
    let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
    let state = Arc::new(AppState::new(config, settings));
    let app = router(Arc::clone(&state));

    // Subscribe two streams, one per mode.
    let gaps_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/events?view=gaps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut gaps_stream = BodyStream::new(gaps_response.into_body());

    let all_response = app
        .oneshot(
            Request::builder()
                .uri("/events?view=all")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut all_stream = BodyStream::new(all_response.into_body());

    // Drain the snapshot off both streams.
    let (gaps_snap, _) = next_event(&mut gaps_stream, Duration::from_secs(2))
        .await
        .expect("gaps snapshot");
    let (all_snap, _) = next_event(&mut all_stream, Duration::from_secs(2))
        .await
        .expect("all snapshot");
    assert_eq!(gaps_snap, "snapshot");
    assert_eq!(all_snap, "snapshot");

    // Mutate disk: add a second covered audiobook. Gaps view still Clean;
    // show-all view now lists the new dimmed row.
    touch(&dir.path().join("Author/Book2/01.mp3"));
    touch(&dir.path().join("Author/Book2/Book2.epub"));

    // Show-all subscriber must receive a section event within the deadline.
    let mut got_all_section = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let Some((name, _data)) = next_event(&mut all_stream, Duration::from_millis(500)).await
        else {
            continue;
        };
        if name == "ping" {
            continue;
        }
        assert_eq!(name, "section", "show-all gets section, not ping or other");
        got_all_section = true;
        break;
    }
    assert!(
        got_all_section,
        "show-all subscriber received a section event"
    );

    // Meanwhile the gaps subscriber must NOT have received a section event.
    // Use a short timeout per read so we drain any pings.
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < drain_deadline {
        let Some((name, _data)) = next_event(&mut gaps_stream, Duration::from_millis(500)).await
        else {
            continue;
        };
        assert!(
            name != "section",
            "gaps subscriber must not receive a section event for a show-all-only change; got: {name}"
        );
    }
}
