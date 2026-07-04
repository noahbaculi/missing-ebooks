//! Production-router SSE invariants: the snapshot carries every section the
//! index would render. A disk change pushes one section event. A quiet
//! library pushes none. A show-all-only change skips the gaps subscriber.
//! Each test bootstraps through `setup` so the four invariants share one
//! binary and one helper.

mod common;

use std::path::PathBuf;
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

use common::{body_to_string, next_event, touch};

/// Build the production router over the given roots and autosync interval.
/// `ttl_seconds` is fixed at 600: high enough to outlive the test.
fn setup(library_roots: Vec<PathBuf>, autosync_interval_seconds: u64) -> (Router, Arc<AppState>) {
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

#[tokio::test]
async fn snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = scenarios::materialize(&(scenario.spec)(), dir.path());
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
    // Last-Event-ID is set so the handler treats this as a reconnect and
    // sends the snapshot. First-connect skips it (ADR-0030).
    let sse_response = app
        .oneshot(
            Request::builder()
                .uri("/events?view=gaps")
                .header("last-event-id", "r")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // nginx buffers proxy responses by default. Without X-Accel-Buffering: no
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
    // The first event on every SSE connection is the ack sentinel (ADR-0030).
    // Drain it so the snapshot assertion below still reads the second event.
    let (ack_name, _) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("ack arrives first");
    assert_eq!(ack_name, "ack", "first SSE event is the ack sentinel");
    let (name, data) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("snapshot arrives after the ack");
    assert_eq!(name, "snapshot", "second SSE event is the snapshot");

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
    let roots = scenarios::materialize(&(scenario.spec)(), dir.path());
    let first_root = roots[0].clone();
    let (app, _state) = setup(roots, 1);

    // Subscribe. Ack arrives first, then the snapshot. Last-Event-ID is sent
    // so the handler treats this as a reconnect and emits the snapshot
    // (ADR-0030).
    let response = app
        .oneshot(
            Request::builder()
                .uri("/events?view=gaps")
                .header("last-event-id", "r")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = BodyStream::new(response.into_body());

    // Drain the ack sentinel that lands first on every SSE connection
    // (ADR-0030), then the snapshot, so the next read is the loop's first
    // scan diff.
    let (ack_name, _) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("ack arrives first");
    assert_eq!(ack_name, "ack");
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
    // when other events flow, but filter just in case.
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
    let roots = scenarios::materialize(&(scenario.spec)(), dir.path());
    let (app, _state) = setup(roots, 1);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/events?view=gaps")
                .header("last-event-id", "r")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = BodyStream::new(response.into_body());

    // Drain the ack sentinel and then the snapshot.
    let (ack_name, _) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("ack arrives first");
    assert_eq!(ack_name, "ack");
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
    // Seed: one covered audiobook. Gaps view is Clean. Show-all renders the
    // covered tree.
    touch(&dir.path().join("Author/Book1/01.mp3"));
    touch(&dir.path().join("Author/Book1/Book1.epub"));

    let (app, _state) = setup(vec![dir.path().to_path_buf()], 1);

    // Subscribe two streams, one per mode. Last-Event-ID is sent on both so
    // the handler emits the snapshot for each (ADR-0030).
    let gaps_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/events?view=gaps")
                .header("last-event-id", "r")
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
                .header("last-event-id", "r")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut all_stream = BodyStream::new(all_response.into_body());

    // Drain the ack sentinel and then the snapshot off both streams.
    let (gaps_ack, _) = next_event(&mut gaps_stream, Duration::from_secs(2))
        .await
        .expect("gaps ack");
    let (all_ack, _) = next_event(&mut all_stream, Duration::from_secs(2))
        .await
        .expect("all ack");
    assert_eq!(gaps_ack, "ack");
    assert_eq!(all_ack, "ack");
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
    // view still Clean with total_audiobooks=1. Show-all view's section
    // re-renders with the extra ebook listed.
    //
    // Adding or removing a whole audiobook is not a show-all-only change
    // anymore: both modes' `RootSection` carry `total_audiobooks`, so the
    // coverage denominator shifts in both. A pure show-all-only change is one
    // that touches state the show-all tree displays (cover/ebook filenames)
    // without altering audiobook count or gap flagging.
    let book_dir = dir.path().join("Author/Book1");
    touch(&book_dir.join("Book1.companion.epub"));
    // Push the folder mtime forward so the autosync walk re-lists Book1 and
    // sees the new ebook regardless of the filesystem's mtime resolution. A
    // same-tick touch would otherwise reuse the cached pre-companion listing
    // and the show-all view would not change.
    filetime::set_file_mtime(
        &book_dir,
        filetime::FileTime::from_unix_time(4_000_000_000, 0),
    )
    .unwrap();

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

#[tokio::test]
async fn first_connect_skips_snapshot() {
    // On first connect the browser has no Last-Event-ID. The page just
    // rendered the same state inline, so the handler must skip the
    // snapshot to avoid redundant work. The ack still lands first to seed
    // the browser's lastEventId for any future reconnect (ADR-0030).
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = scenarios::materialize(&(scenario.spec)(), dir.path());
    let (app, _state) = setup(roots, 60);

    let sse_response = app
        .oneshot(
            Request::builder()
                .uri("/events?view=gaps")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = BodyStream::new(sse_response.into_body());

    let (first_name, _first_data) = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("ack arrives first");
    assert_eq!(first_name, "ack", "first SSE event is the ack sentinel");

    // No second event within a brief window: the snapshot was skipped.
    let arrived = next_event(&mut stream, Duration::from_millis(200))
        .await
        .map(|(name, _)| name);
    assert!(
        arrived.is_none(),
        "first connect must not receive a snapshot; got {arrived:?}"
    );
}
