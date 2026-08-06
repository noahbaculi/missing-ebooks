//! The MarkOverlay: a borrowing view over the session's mark set that the
//! demo render path consults per folder, replacing the clone-and-replay
//! `derive_view` shape. The semantic oracle is `crate::raw_view::apply_mark_raw`.
//! The equivalence test pins byte-for-byte parity.

use std::collections::HashSet;
use std::path::Path;

use crate::demo::session::MarkKey;
use crate::marker::Marker;
use crate::raw_view::RawView;

/// Borrowing view over a session's mark set. Answers per-folder questions
/// about which marks apply and how, without cloning the set.
pub(crate) struct MarkOverlay<'a> {
    marks: &'a HashSet<MarkKey>,
}

/// What the overlay says about one folder: whether any ancestor mark cleared
/// it, and which marker filenames apply to the folder itself (in
/// `Marker::ALL` declaration order).
#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct EffectiveState {
    /// Set when a mark on this folder or any ancestor would flip
    /// `missing_ebook` to false.
    pub cleared_by_ancestor: bool,
    /// Marker filenames to append to the folder's `cover_files`, in
    /// `Marker::ALL` declaration order so the result matches
    /// `apply_mark_raw`'s output for the canonical replay order.
    pub exact_markers: Vec<Marker>,
}

impl<'a> MarkOverlay<'a> {
    /// Borrow the session's mark set into a new overlay.
    pub fn new(marks: &'a HashSet<MarkKey>) -> Self {
        Self { marks }
    }

    /// Compute the overlay-corrected state for the folder at `(root, rel)`.
    ///
    /// Walks `rel`'s ancestors (including itself) and probes every
    /// `Marker::ALL` kind. A hit on any ancestor sets `cleared_by_ancestor`.
    /// A hit on `rel` itself also appends the marker to `exact_markers` in
    /// `Marker::ALL` declaration order, matching `apply_mark_raw`'s
    /// `add_marker` output for the same canonical replay order.
    ///
    /// Depth is typically 2-3 in audiobook libraries, so this is `O(depth)`
    /// `HashSet` probes per folder.
    pub fn effective_state(&self, root: usize, rel: &Path) -> EffectiveState {
        let mut state = EffectiveState::default();

        for ancestor in rel.ancestors() {
            let ancestor_key: String = if ancestor.as_os_str().is_empty() {
                ".".to_string()
            } else {
                match ancestor.to_str() {
                    Some(s) => s.to_string(),
                    None => continue,
                }
            };

            // Iterate Marker::ALL in declaration order so the exact_markers
            // vec, when consumed by package_view_with_overlay, appends to
            // cover_files in the same order apply_mark_raw would.
            for kind in Marker::ALL {
                let key = (root, ancestor_key.clone(), kind);
                if self.marks.contains(&key) {
                    state.cleared_by_ancestor = true;
                    if ancestor == rel {
                        state.exact_markers.push(kind);
                    }
                }
            }
        }

        state
    }
}

/// Materialize the overlay against `base` into a fresh `RawView`. Walks
/// `base` once, cloning each section and applying per-folder overlay
/// edits in place. Returns a raw view the demo handlers hand to the
/// shared render seams (`page`, `packaged_section`, `all_sections`), so
/// the overlay path inherits any future refinement to packaging or
/// rendering for free.
///
/// Cost: `O(F)` clone plus `O(F * depth)` overlay probes.
pub(crate) fn package_view_with_overlay(base: &RawView, overlay: &MarkOverlay<'_>) -> RawView {
    let mut synthesized = base.clone();
    for (root_idx, section) in synthesized.iter_mut().enumerate() {
        let crate::scanner::RootScan::Walked { folders, .. } = section else {
            continue;
        };
        for folder in folders.iter_mut() {
            let state = overlay.effective_state(root_idx, &folder.rel_path);
            if state.cleared_by_ancestor {
                folder.missing_ebook = false;
            }
            for marker in state.exact_markers {
                let name = marker.filename().to_string();
                if !folder.cover_files.iter().any(|existing| existing == &name) {
                    // Same rebuild-on-mark pattern as raw_view::add_marker: the
                    // cover list is shared via Arc<[String]>, so a synthesized
                    // marker drops a fresh allocation in place of the shared one.
                    let mut next: Vec<String> = folder.cover_files.to_vec();
                    next.push(name);
                    folder.cover_files = next.into();
                }
            }
        }
    }
    synthesized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::scanner::ScanSettings;
    use crate::scenarios;
    use crate::state::AppState;
    use crate::tree::ViewMode;
    use crate::web::render;
    use std::sync::Arc;

    /// For each interesting scenario and mark set: compare the overlay
    /// render to a fresh `apply_mark_raw`-replay render. The HTML must be
    /// byte-equal. This is the merge gate for Task 5: if it fails, do not
    /// land.
    #[tokio::test]
    async fn overlay_matches_replay_render_byte_for_byte() {
        for scenario_name in ["mixed-forest", "messy-shelf", "root-flagged", "pre-marked"] {
            for case in interesting_mark_sets() {
                assert_byte_equal(scenario_name, &case).await;
            }
        }
    }

    struct Case {
        name: &'static str,
        marks: Vec<MarkKey>,
    }

    fn interesting_mark_sets() -> Vec<Case> {
        // Returns logical mark-sets. The fixtures-bound rel paths must
        // exist in the scenario. A missing path is silently filtered out
        // inside assert_byte_equal so the equivalence is only measured on
        // marks both paths can apply.
        vec![
            Case {
                name: "empty",
                marks: vec![],
            },
            Case {
                name: "single_leaf",
                marks: vec![(0, "Author/Book".to_string(), Marker::NoEbook)],
            },
            Case {
                name: "single_root_dot",
                // ADR-0007 root mark.
                marks: vec![(0, ".".to_string(), Marker::NoEbook)],
            },
            Case {
                name: "ancestor_plus_descendant",
                marks: vec![
                    (0, "Author".to_string(), Marker::NoEbook),
                    (0, "Author/Book".to_string(), Marker::EbookElsewhere),
                ],
            },
            Case {
                name: "both_markers_on_one_folder",
                marks: vec![
                    (0, "Author/Book".to_string(), Marker::NoEbook),
                    (0, "Author/Book".to_string(), Marker::EbookElsewhere),
                ],
            },
        ]
    }

    async fn assert_byte_equal(scenario_name: &str, case: &Case) {
        let dir = tempfile::tempdir().unwrap();
        let scenario = scenarios::find_scenario(scenario_name).expect("scenario exists");
        let roots = scenarios::materialize(&(scenario.spec)(), dir.path());
        let config = Config {
            library_roots: roots,
            ttl_seconds: 600,
            ..Config::default()
        };
        let links = config.search_links.clone();
        let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
        let state = Arc::new(AppState::new(config, settings));
        // Warm the cache so .current() returns a stable raw view.
        let base = state.store.current().await;

        for mode in [ViewMode::GapsOnly, ViewMode::All] {
            // Path A: replay marks via apply_mark_raw, then page().
            // This is the production-equivalent path.
            let mut raw_replay = (*base).clone();
            // Filter for valid folder rels so a missing-from-scenario mark
            // does not silently no-op the replay. The equivalence is only
            // meaningful when both paths see the same logical state.
            let valid_marks: Vec<&MarkKey> = case
                .marks
                .iter()
                .filter(|(root, rel, _)| folder_in_raw(&raw_replay, *root, rel))
                .collect();
            for (root, rel, kind) in &valid_marks {
                crate::raw_view::apply_mark_raw(&mut raw_replay, *root, rel, *kind);
            }
            let replay_html = render::page(&raw_replay, &links, mode, 0).into_string();

            // Path B: same logical state via the overlay.
            let mark_set: HashSet<MarkKey> = valid_marks.iter().map(|k| (*k).clone()).collect();
            let overlay = MarkOverlay::new(&mark_set);
            let overlay_raw = package_view_with_overlay(&base, &overlay);
            let overlay_html = render::page(&overlay_raw, &links, mode, 0).into_string();

            assert_eq!(
                replay_html, overlay_html,
                "scenario={scenario_name} case={} mode={mode:?}: overlay HTML diverges from replay HTML",
                case.name
            );
        }
    }

    fn folder_in_raw(raw: &RawView, root: usize, rel: &str) -> bool {
        let Some(crate::scanner::RootScan::Walked { folders, .. }) = raw.get(root) else {
            return false;
        };
        if rel == "." {
            return true;
        }
        let target = std::path::PathBuf::from(rel);
        folders.iter().any(|f| f.rel_path == target)
    }
}
