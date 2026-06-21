//! With no disk changes, the SSE stream stays silent: only the initial
//! `snapshot` and `ping` events arrive. A regression that re-emits every
//! section on the loop's first tick (because `last_hash` was not seeded from
//! the snapshot) would fail this test.

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

use common::next_event;

#[tokio::test]
async fn quiet_library_emits_no_section_events() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = (scenario.build)(dir.path());

    let config = Config {
        library_roots: roots,
        ttl_seconds: 600,
        autosync_interval_seconds: 1,
        ..Config::default()
    };
    let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
    let state = Arc::new(AppState::new(config, settings));
    let app = router(Arc::clone(&state));

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

    // Drain the snapshot.
    let (snapshot_name, _) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("snapshot arrives");
    assert_eq!(snapshot_name, "snapshot");

    // Drain events for the next ~2.5 seconds (long enough for the loop to
    // tick twice on a 1 s interval). Nothing should be a `section` event.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
    while tokio::time::Instant::now() < deadline {
        // Read at most one event per loop with a short per-event timeout, so
        // a quiet stretch doesn't block the overall deadline forever.
        let event = next_event(&mut stream, Duration::from_millis(500)).await;
        let Some((name, _data)) = event else {
            // No event within the slice: loop again until the deadline.
            continue;
        };
        assert!(
            name != "section",
            "no section events should fire on a quiet library; got: {name}"
        );
    }
}
