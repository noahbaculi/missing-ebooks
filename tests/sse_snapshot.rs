//! The first SSE event a subscriber receives is the snapshot, and its payload
//! carries every root section the index page would render.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyStream;
use missing_ebooks::config::Config;
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::scenarios;
use missing_ebooks::state::AppState;
use missing_ebooks::web::router;
use tower::ServiceExt;

use common::{body_to_string, next_event};

#[tokio::test]
async fn first_event_is_snapshot_matching_index_render() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = (scenario.build)(dir.path());
    let config = Config {
        library_roots: roots,
        ttl_seconds: 600,
        autosync_interval_seconds: 60,
        ..Config::default()
    };
    let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
    let state = Arc::new(AppState::new(config, settings));
    let app = router(Arc::clone(&state));

    // The index render carries every section, each tagged with its id.
    let index_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/?view=gaps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let index_html = body_to_string(index_response.into_body()).await;

    // The SSE snapshot must carry every section id the index page does.
    let sse_response = app
        .oneshot(
            Request::builder()
                .uri("/events?view=gaps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // nginx buffers proxy responses by default; without X-Accel-Buffering: no
    // the autosync ticks accumulate at the proxy and arrive in bursts. axum's
    // Sse does not set this for us.
    assert_eq!(
        sse_response
            .headers()
            .get("x-accel-buffering")
            .and_then(|v| v.to_str().ok()),
        Some("no"),
        "X-Accel-Buffering: no must be set so nginx does not buffer SSE",
    );
    let mut stream = BodyStream::new(sse_response.into_body());
    let (name, data) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("first event arrives");
    assert_eq!(name, "snapshot", "first SSE event is the snapshot");

    // Each `id="root-N-section"` appearing in the index page must appear in
    // the snapshot too. Walk root indices until one is absent from the index.
    for section_id in 0.. {
        let needle = format!("id=\"root-{section_id}-section\"");
        if !index_html.contains(&needle) {
            assert!(section_id > 0, "scenario should have at least one section");
            break;
        }
        assert!(
            data.contains(&needle),
            "snapshot must carry section {section_id}"
        );
    }
}
