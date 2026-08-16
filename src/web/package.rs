//! Raw-to-packaged step for the web read view. Owns `RootSection`,
//! `FlaggedView`, and the `SectionHandle` that `web.rs` hands to inline
//! swaps. `render.rs` renders these types and never sees `RootScan`.

use maud::Markup;

use crate::config::SearchLink;
use crate::raw_view::RawView;
use crate::scanner::RootScan;
use crate::tree;
use crate::tree::{RootState, ViewMode};

/// The whole read view: one section per configured library root, in config order.
pub(super) type FlaggedView = Vec<RootSection>;

/// One library root's outcome, labeled with the path the scanner walked.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct RootSection {
    /// The canonical root path when it resolved, else the configured path.
    pub(super) path: String,
    /// What the scan found for this root.
    pub(super) state: RootState,
    /// Folders under this root that directly hold audio. Zero for `Clean` and
    /// `Error`. The web layer surfaces it as `data-total-audiobooks` on the
    /// section so the strip's library coverage stays current across swaps.
    pub(super) total_audiobooks: usize,
    /// Total gaps across this root's forest, precomputed by `build_forest` so
    /// the summary strip and per-root chip read a number instead of walking
    /// the tree. `Clean` and `Error` are zero by construction.
    pub(super) gaps_within: usize,
    /// Directories the walk could not read for this root. Nonzero renders
    /// the "couldn't be read" partial-scan warning strip.
    pub(super) skipped_dirs: usize,
    /// Subtree roots this root's walk pruned via the depth cap. Nonzero
    /// renders a separate "depth limit" warning strip, since the cause
    /// (a hardcoded ceiling, not a read failure) and remediation differ
    /// from `skipped_dirs`.
    pub(super) depth_capped_dirs: usize,
}

/// Build the per-mode `FlaggedView` from the cached raw scan output. The gaps
/// path filters with `reduce_to_flagged` and builds the forest. Show-all builds
/// directly from the raw folders. Both run on the request thread (the per-folder
/// cost is bounded, see ADR-0022). Allocates a fresh `FlaggedView` per response
/// and drops it after the response writes.
pub(super) fn package_view(raw: &RawView, mode: ViewMode) -> FlaggedView {
    raw.iter().map(|scan| package_section(scan, mode)).collect()
}

/// Build one `RootSection` from a raw `RootScan` for the requested mode.
///
/// The single owner of the raw-to-packaged step. `package_view` calls it on
/// the snapshot path; `packaged_section` calls it to build a `SectionHandle`
/// for every mark/unmark response. Any future per-root field lands here
/// once.
pub(super) fn package_section(scan: &RootScan, mode: ViewMode) -> RootSection {
    let state = tree::build(scan, mode);
    let gaps_within = match &state {
        RootState::Forest(nodes) => nodes.iter().map(|n| n.gaps_within).sum(),
        RootState::Clean | RootState::Error(_) => 0,
    };
    RootSection {
        path: scan.display_path().to_string(),
        state,
        total_audiobooks: scan.audiobook_count(),
        gaps_within,
        skipped_dirs: match scan {
            RootScan::Walked { skipped_dirs, .. } => *skipped_dirs,
            RootScan::Failed { .. } => 0,
        },
        depth_capped_dirs: match scan {
            RootScan::Walked {
                depth_capped_dirs, ..
            } => *depth_capped_dirs,
            RootScan::Failed { .. } => 0,
        },
    }
}

/// One packaged section plus the identifying context needed to render it.
/// Constructed by `packaged_section`, which owns the raw → packaged step.
/// The handle owns its `RootSection` so callers do not name intermediate
/// types.
pub struct SectionHandle {
    section: RootSection,
    root: usize,
    mode: ViewMode,
}

impl SectionHandle {
    /// Render the section for an inline swap. `alert` shows as an
    /// in-section error banner when `Some`.
    #[must_use]
    pub fn render(&self, links: &[SearchLink], alert: Option<&str>) -> Markup {
        super::render::render_section(&self.section, self.root, alert, links, self.mode)
    }
}

/// Package one root's section from `raw`, ready to render. Panics if
/// `root >= raw.len()`; callers validate the index before reaching this
/// seam (`WriteFailure::BadRoot` in `web::mark`/`unmark`, an explicit
/// bounds check in `demo::apply_mark`).
#[must_use]
pub fn packaged_section(raw: &RawView, root: usize, mode: ViewMode) -> SectionHandle {
    let section = package_section(&raw[root], mode);
    SectionHandle {
        section,
        root,
        mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Config;
    use crate::raw_view::build_view;
    use crate::scanner::{DirIndex, ScanSettings};
    use crate::scenarios::touch;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_config(roots: Vec<PathBuf>, scan_cache_ttl_seconds: u64) -> Config {
        Config {
            library_roots: roots,
            scan_cache_ttl_seconds,
            ..Default::default()
        }
    }

    fn test_settings() -> Arc<ScanSettings> {
        Arc::new(ScanSettings::compile(Config::default().scan_inputs()).unwrap())
    }

    fn test_indices(roots: usize) -> Vec<Arc<DirIndex>> {
        (0..roots).map(|_| Arc::new(DirIndex::new())).collect()
    }

    #[tokio::test]
    async fn package_view_root_with_a_gap_yields_a_matching_forest() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), &test_indices(1)).await;
        let view = package_view(&raw, ViewMode::GapsOnly);
        assert_eq!(view.len(), 1);
        match &view[0].state {
            RootState::Forest(nodes) => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].name, "Author");
            }
            other => panic!("expected Forest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn package_view_root_with_no_audio_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Empty")).unwrap();
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), &test_indices(1)).await;
        let view = package_view(&raw, ViewMode::GapsOnly);
        assert!(matches!(view[0].state, RootState::Clean));
    }

    #[tokio::test]
    async fn package_view_missing_root_is_error_and_other_roots_still_render() {
        let good = tempfile::tempdir().unwrap();
        touch(&good.path().join("Book/01.mp3"));
        let cfg = test_config(
            vec![
                PathBuf::from("/no/such/root/xyz123"),
                good.path().to_path_buf(),
            ],
            60,
        );
        let raw = build_view(&cfg, &test_settings(), &test_indices(2)).await;
        let view = package_view(&raw, ViewMode::GapsOnly);
        assert!(matches!(view[0].state, RootState::Error(_)));
        assert!(matches!(view[1].state, RootState::Forest(_)));
    }

    #[tokio::test]
    async fn package_view_computes_total_audiobooks_per_root() {
        let walked = tempfile::tempdir().unwrap();
        // Two audiobooks under one author, plus a covered one.
        touch(&walked.path().join("A/B1/01.mp3"));
        touch(&walked.path().join("A/B2/01.mp3"));
        touch(&walked.path().join("A/B2/B2.epub"));

        let clean = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(clean.path().join("Empty")).unwrap();

        let cfg = test_config(
            vec![
                walked.path().to_path_buf(),
                clean.path().to_path_buf(),
                PathBuf::from("/no/such/root/xyz123"),
            ],
            60,
        );
        let raw = build_view(&cfg, &test_settings(), &test_indices(3)).await;
        let view = package_view(&raw, ViewMode::GapsOnly);

        assert_eq!(view[0].total_audiobooks, 2, "two audiobook folders");
        assert_eq!(view[1].total_audiobooks, 0, "clean root");
        assert_eq!(view[2].total_audiobooks, 0, "errored root");
    }

    #[tokio::test]
    async fn package_view_all_mode_builds_the_full_tree_including_covered_folders() {
        let dir = tempfile::tempdir().unwrap();
        // A gap and a covered book under the same author.
        touch(&dir.path().join("Author/Gap/01.mp3"));
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), &test_indices(1)).await;
        let view = package_view(&raw, ViewMode::All);
        let RootState::Forest(nodes) = &view[0].state else {
            panic!("show-all always yields a Forest");
        };
        let author = &nodes[0];
        assert_eq!(author.name, "Author");
        let names: Vec<&str> = author.children.iter().map(|n| n.name.as_str()).collect();
        // Both books appear, unlike gaps-only which would drop Covered.
        assert_eq!(names, vec!["Covered", "Gap"]);
    }
}
