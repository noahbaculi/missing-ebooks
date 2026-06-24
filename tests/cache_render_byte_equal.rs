//! End-to-end pin: render-on-read produces stable bytes across cache hits,
//! mode flips, and a mark + unmark round trip on a real seeded scenario. A
//! future change that breaks render equivalence trips this test, not a
//! snapshot diff in review.

use std::sync::Arc;

use missing_ebooks::config::Config;
use missing_ebooks::marker::Marker;
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::scenarios;
use missing_ebooks::service::{current_view, mark, unmark};
use missing_ebooks::state::AppState;
use missing_ebooks::tree::ViewMode;
use missing_ebooks::web::render::{oob_sections, render_section};

#[tokio::test]
async fn render_is_byte_equal_across_hits_and_a_mark_undo_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
    let roots = (scenario.build)(dir.path());

    let config = Config {
        library_roots: roots,
        ttl_seconds: 600,
        ..Config::default()
    };
    // Clone search_links before the config is consumed by AppState::new so the
    // SSE-payload assertion below has a borrow to pass into render helpers.
    let links = config.search_links.clone();
    let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
    let state = Arc::new(AppState::new(config, settings));

    // Two reads of the same mode on a warm cache must serialize identically.
    let gaps_one = current_view(&state, ViewMode::GapsOnly).await;
    let gaps_two = current_view(&state, ViewMode::GapsOnly).await;
    assert_eq!(
        serde_json::to_vec(&*gaps_one).unwrap(),
        serde_json::to_vec(&*gaps_two).unwrap(),
        "two reads of the same mode must produce byte-equal renders",
    );

    // A mode flip on the same warm cache must produce a different shape
    // (otherwise the test would not catch a swapped mode).
    let all_one = current_view(&state, ViewMode::All).await;
    assert_ne!(
        serde_json::to_vec(&*gaps_one).unwrap(),
        serde_json::to_vec(&*all_one).unwrap(),
        "gaps and show-all must render to different bytes on a non-clean scenario",
    );

    // Pick the first flagged leaf the scenario exposes, mark it, then undo.
    // After undo the gaps view must match the pre-mark gaps view byte-for-byte.
    let (root_idx, rel) = first_flagged(&gaps_one).expect("scenario has at least one gap");
    let outcome = mark(&state, root_idx, &rel, Marker::NoEbook, ViewMode::GapsOnly)
        .await
        .expect("mark succeeds");
    assert!(outcome.created, "the picked leaf was not already marked");
    let after_mark = serde_json::to_vec(&*outcome.view).unwrap();
    assert_ne!(
        serde_json::to_vec(&*gaps_one).unwrap(),
        after_mark,
        "the mark must change the gaps view",
    );

    let restored = unmark(&state, root_idx, &rel, Marker::NoEbook, ViewMode::GapsOnly)
        .await
        .expect("unmark succeeds");
    assert_eq!(
        serde_json::to_vec(&*gaps_one).unwrap(),
        serde_json::to_vec(&*restored).unwrap(),
        "undoing the mark must restore the gaps view byte-for-byte",
    );

    // The OOB-wrapped snapshot payload must contain the byte-for-byte fragment
    // a direct render_section produces, so an SSE subscriber and a Rescan
    // click see identical bytes for the same root (ADR-0024).
    let direct = render_section(&restored[0], 0, None, &links, ViewMode::GapsOnly).into_string();
    let snapshot = oob_sections(&restored, &links, ViewMode::GapsOnly).into_string();
    assert!(
        snapshot.contains(&direct),
        "the OOB-wrapped snapshot must contain the same root-0 fragment as a direct render"
    );
}

/// Walk the rendered gaps view and return the (root_index, rel_path) of the
/// first folder whose state names a flagged node. The scenario picker for the
/// test depends only on the rendered shape, so it survives any future change
/// to the seeded fixture so long as at least one gap remains.
fn first_flagged(view: &missing_ebooks::service::FlaggedView) -> Option<(usize, String)> {
    use missing_ebooks::tree::Node;
    use missing_ebooks::tree::RootState;
    fn first_leaf(node: &Node) -> Option<String> {
        if node.directly_holds_audio && node.missing_ebook {
            return Some(node.rel_path.clone());
        }
        node.children.iter().find_map(first_leaf)
    }
    for (idx, section) in view.iter().enumerate() {
        if let RootState::Forest(nodes) = &section.state
            && let Some(rel) = nodes.iter().find_map(first_leaf)
        {
            return Some((idx, rel));
        }
    }
    None
}
