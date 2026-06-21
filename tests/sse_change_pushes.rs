//! After a change on disk and one autosync tick, a `section` event carrying
//! the changed root's OOB target arrives within `interval + slop` of the
//! mutation.

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
async fn change_on_disk_pushes_one_section_event() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = (scenario.build)(dir.path());
    let first_root = roots[0].clone();

    let config = Config {
        library_roots: roots,
        ttl_seconds: 600,
        autosync_interval_seconds: 1,
        ..Config::default()
    };
    let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
    let state = Arc::new(AppState::new(config, settings));
    let app = router(Arc::clone(&state));

    // Subscribe; the snapshot arrives first.
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

    // Drain the snapshot so the next read is the loop's first scan diff.
    let (snapshot_name, _) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("snapshot arrives");
    assert_eq!(snapshot_name, "snapshot");

    // Mutate disk: drop a new audio file directly under the first root so the
    // root section's HTML changes (a new "loose at top" gap appears).
    let new_audio = first_root.join("trigger-autosync.mp3");
    std::fs::write(&new_audio, b"").unwrap();

    // Within interval + slop, a `section` event should arrive carrying the
    // mutated root's OOB target. The `ping` keep-alive is suppressed by axum
    // when other events flow; we filter just in case.
    let mut got_section = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let Some((name, data)) = next_event(&mut stream, Duration::from_secs(3)).await else {
            break;
        };
        if name == "ping" {
            continue;
        }
        assert_eq!(name, "section", "the change triggered a section push");
        assert!(
            data.contains("#root-0-section"),
            "section event targets root 0; got: {}",
            &data[..data.len().min(200)]
        );
        got_section = true;
        break;
    }
    assert!(got_section, "a section event arrived within the deadline");
}
