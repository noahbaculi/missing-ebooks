//! The demo's /events endpoint serves exactly one snapshot and stays silent
//! after that. ADR-0023 documents the carve-out (the session sweep's idle
//! signal does not track SSE traffic yet, so the demo disables the loop
//! entirely). This test pins it so a future change that wires the demo to a
//! real autosync loop fails loudly here.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyStream;
use missing_ebooks::config::Config;
use missing_ebooks::demo::handlers::router;
use missing_ebooks::demo::state::{DemoConfig, build_state};
use missing_ebooks::scanner::ScanSettings;
use tower::ServiceExt;

use common::{next_event, touch};

#[tokio::test]
async fn demo_events_emits_one_snapshot_then_stays_silent() {
    let dir = tempfile::tempdir().unwrap();
    // One flagged folder so the snapshot has content to render.
    touch(&dir.path().join("Author/Book/01.mp3"));

    let cfg = Config {
        library_roots: vec![dir.path().to_path_buf()],
        ttl_seconds: 60,
        autosync_interval_seconds: 0,
        ..Config::default()
    };
    let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
    let demo_config = DemoConfig {
        bind: "127.0.0.1:0".to_string(),
        scenario: "test".to_string(),
        max_sessions: 4,
        idle: Duration::from_secs(60),
        cookie_name: "me_demo_sid".to_string(),
    };
    let state = Arc::new(build_state(cfg, settings, demo_config).await);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/events?view=gaps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = BodyStream::new(response.into_body());

    let (name, data) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("snapshot arrives");
    assert_eq!(name, "snapshot", "first event is the snapshot");
    assert!(
        data.contains("id=\"root-0-section\""),
        "snapshot carries the seeded root's section",
    );

    // The demo runs no autosync loop, so no section events follow. Drain
    // anything that arrives within a short deadline and assert none of it is
    // a section event. Only ping keepalives are acceptable.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
    while tokio::time::Instant::now() < deadline {
        let event = next_event(&mut stream, Duration::from_millis(500)).await;
        let Some((name, _data)) = event else {
            continue;
        };
        assert!(
            name != "section",
            "demo must not emit section events; got: {name}",
        );
    }
}
