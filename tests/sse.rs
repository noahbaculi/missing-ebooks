//! Production-router SSE invariants: the snapshot carries every section the
//! index would render; a disk change pushes one section event; a quiet
//! library pushes none; a show-all-only change skips the gaps subscriber.
//! Each test bootstraps through `setup` so the four invariants share one
//! binary and one helper.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
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

/// Build the production router over the given roots and autosync interval.
/// `ttl_seconds` is fixed at 600 because every pre-merge binary used 600;
/// no other `Config` field is overridden, matching every pre-merge build.
fn setup(
    library_roots: Vec<PathBuf>,
    autosync_interval_seconds: u64,
) -> (Router, Arc<AppState>) {
    let config = Config {
        library_roots,
        ttl_seconds: 600,
        autosync_interval_seconds,
        ..Config::default()
    };
    let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
    let state = Arc::new(AppState::new(config, settings));
    let app = router(Arc::clone(&state));
    (app, state)
}

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"").unwrap();
}

#[tokio::test]
async fn snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = (scenario.build)(dir.path());
    let (app, _state) = setup(roots, 60);

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

#[tokio::test]
async fn change_pushes() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = (scenario.build)(dir.path());
    let first_root = roots[0].clone();
    let (app, _state) = setup(roots, 1);

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

#[tokio::test]
async fn no_change_silent() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = (scenario.build)(dir.path());
    let (app, _state) = setup(roots, 1);

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

#[tokio::test]
async fn two_modes_isolated() {
    let dir = tempfile::tempdir().unwrap();
    // Seed: one covered audiobook. Gaps view is Clean; show-all renders the
    // covered tree.
    touch(&dir.path().join("Author/Book1/01.mp3"));
    touch(&dir.path().join("Author/Book1/Book1.epub"));

    let (app, _state) = setup(vec![dir.path().to_path_buf()], 1);

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

    // Mutate disk: add a second ebook file to the covered audiobook. This
    // extends `cover_files` (visible in the show-all tree) without changing
    // `directly_holds_audio`, `missing_ebook`, or the audiobook count. Gaps
    // view still Clean with total_audiobooks=1; show-all view's section
    // re-renders with the extra ebook listed.
    //
    // Adding or removing a whole audiobook is not a show-all-only change
    // anymore: both modes' `RootSection` carry `total_audiobooks`, so the
    // coverage denominator shifts in both. A pure show-all-only change is one
    // that touches state the show-all tree displays (cover/ebook filenames)
    // without altering audiobook count or gap flagging.
    touch(&dir.path().join("Author/Book1/Book1.companion.epub"));

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
