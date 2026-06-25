//! Web-agnostic service layer: the read view types and the typed operations
//! (current view, marker write) shared by the HTML UI and a future JSON API.

use std::sync::Arc;

use crate::marker::Marker;
use crate::state::AppState;
use crate::tree::ViewMode;

pub use crate::state::DomainError;
pub use crate::web::render::{FlaggedView, RootSection};
// Pre-fold helper names. The new homes are package_view and package_section in
// web::render. These re-exports go away with service.rs itself.
pub(crate) use crate::web::render::package_section as render_section_from_raw;
pub(crate) use crate::web::render::package_view as render_view;

/// Return the cached view if it is still fresh, otherwise scan and cache it.
/// Single-flight is enforced by `RawViewStore::current`.
pub async fn current_view(state: &AppState, mode: ViewMode) -> Arc<FlaggedView> {
    let raw = state.store.current().await;
    Arc::new(render_view(&raw, mode))
}

/// Force a fresh cold scan by clearing the dir index and rebuilding from
/// scratch. Ignores the TTL. This is the explicit "fix any drift" path; the
/// autosync loop keeps using warm scans (see ADR-0023).
pub async fn rescan(state: &AppState, mode: ViewMode) -> Arc<FlaggedView> {
    let raw = state.store.rescan().await;
    Arc::new(render_view(&raw, mode))
}

/// The result of a marker write: the refreshed view plus whether this call
/// actually created the file. `created` is false for a re-mark of an
/// already-marked folder, which the HTML surface uses to suppress the undo toast.
#[derive(Debug)]
pub struct MarkOutcome {
    /// The refreshed view after the write, the requesting mode's slot.
    pub view: Arc<FlaggedView>,
    /// True when this call made the file; false for a re-mark of a marked folder.
    pub created: bool,
}

/// Write a marker into a folder and update the cached view in place, without a
/// rescan (see docs/adr/0002-marker-writes-edit-cache-in-place.md). The guard and write run
/// off the cache lock, which is held only for the in-memory mutation.
pub async fn mark(
    state: &AppState,
    root: usize,
    rel: &str,
    marker: Marker,
    mode: ViewMode,
) -> Result<MarkOutcome, DomainError> {
    let applied = state.store.write_mark(root, rel, marker).await?;
    Ok(MarkOutcome {
        view: Arc::new(render_view(&applied.raw, mode)),
        created: applied.created,
    })
}

/// Delete a marker file and refresh the cached view by rescanning the one
/// affected root (see docs/adr/0002-marker-writes-edit-cache-in-place.md). The guard and
/// delete run off the cache lock, which is held only for the per-root rebuild.
pub async fn unmark(
    state: &AppState,
    root: usize,
    rel: &str,
    marker: Marker,
    mode: ViewMode,
) -> Result<Arc<FlaggedView>, DomainError> {
    let raw = state.store.remove_mark(root, rel, marker).await?;
    Ok(Arc::new(render_view(&raw, mode)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    use crate::config::Config;
    use crate::scanner::{self, ScanSettings};
    use crate::scenarios::touch;
    use crate::state::build_view;
    use crate::tree::RootState;

    fn test_config(roots: Vec<PathBuf>, ttl_seconds: u64) -> Config {
        Config {
            library_roots: roots,
            ttl_seconds,
            ..Default::default()
        }
    }

    fn test_settings() -> Arc<ScanSettings> {
        Arc::new(ScanSettings::compile(Config::default().scan_inputs()).unwrap())
    }

    fn test_index() -> Arc<std::sync::Mutex<scanner::DirIndex>> {
        Arc::new(std::sync::Mutex::new(scanner::DirIndex::new()))
    }

    #[tokio::test]
    async fn root_with_a_gap_yields_a_matching_forest() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::GapsOnly);
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
    async fn root_with_no_audio_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Empty")).unwrap();
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::GapsOnly);
        assert!(matches!(view[0].state, RootState::Clean));
    }

    #[tokio::test]
    async fn missing_root_is_error_and_other_roots_still_render() {
        let good = tempfile::tempdir().unwrap();
        touch(&good.path().join("Book/01.mp3"));
        let cfg = test_config(
            vec![
                PathBuf::from("/no/such/root/xyz123"),
                good.path().to_path_buf(),
            ],
            60,
        );
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::GapsOnly);
        assert!(matches!(view[0].state, RootState::Error(_)));
        assert!(matches!(view[1].state, RootState::Forest(_)));
    }

    #[test]
    fn audiobook_count_counts_walked_folders_that_directly_hold_audio() {
        use crate::scanner::{RootScan, ScannedFolder};
        use std::path::PathBuf;

        let walked = RootScan::Walked {
            canonical_path: PathBuf::from("/lib"),
            folders: vec![
                ScannedFolder {
                    rel_path: PathBuf::from("A/Book1"),
                    directly_holds_audio: true,
                    missing_ebook: true,
                    cover_files: Vec::new(),
                    audio_files: vec!["01.mp3".to_string()],
                },
                ScannedFolder {
                    rel_path: PathBuf::from("A"),
                    directly_holds_audio: false,
                    missing_ebook: false,
                    cover_files: Vec::new(),
                    audio_files: Vec::new(),
                },
                ScannedFolder {
                    rel_path: PathBuf::from("A/Book2"),
                    directly_holds_audio: true,
                    missing_ebook: false,
                    cover_files: vec!["Book2.epub".to_string()],
                    audio_files: vec!["01.mp3".to_string()],
                },
            ],
        };
        assert_eq!(walked.audiobook_count(), 2);
        let empty_walked = RootScan::Walked {
            canonical_path: PathBuf::from("/lib"),
            folders: Vec::new(),
        };
        assert_eq!(empty_walked.audiobook_count(), 0);
        let failed = RootScan::Failed {
            path: PathBuf::from("/lib"),
            message: "nope".to_string(),
        };
        assert_eq!(failed.audiobook_count(), 0);
    }

    #[tokio::test]
    async fn render_view_computes_total_audiobooks_per_root() {
        let walked = tempfile::tempdir().unwrap();
        // Two audiobooks under one author, plus a covered one.
        touch(&walked.path().join("A/B1/01.mp3"));
        touch(&walked.path().join("A/B2/01.mp3"));
        touch(&walked.path().join("A/B2/B2.epub"));

        let clean = tempfile::tempdir().unwrap();
        fs::create_dir_all(clean.path().join("Empty")).unwrap();

        let cfg = test_config(
            vec![
                walked.path().to_path_buf(),
                clean.path().to_path_buf(),
                PathBuf::from("/no/such/root/xyz123"),
            ],
            60,
        );
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::GapsOnly);

        assert_eq!(view[0].total_audiobooks, 2, "two audiobook folders");
        assert_eq!(view[1].total_audiobooks, 0, "clean root");
        assert_eq!(view[2].total_audiobooks, 0, "errored root");
    }

    #[test]
    fn root_states_serialize_to_stable_json() {
        let clean = serde_json::to_value(RootState::Clean).unwrap();
        assert_eq!(clean, serde_json::json!("clean"));

        let err = serde_json::to_value(RootState::Error("nope".to_string())).unwrap();
        assert_eq!(err, serde_json::json!({ "error": "nope" }));

        let section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Clean,
            total_audiobooks: 0,
        };
        let value = serde_json::to_value(&section).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "path": "/lib", "state": "clean", "total_audiobooks": 0 })
        );
    }

    #[test]
    fn view_mode_parses_the_query_token_leniently() {
        assert_eq!(ViewMode::from_query(Some("all")), ViewMode::All);
        assert_eq!(ViewMode::from_query(Some("gaps")), ViewMode::GapsOnly);
        // Absent or unrecognized falls back to gaps-only.
        assert_eq!(ViewMode::from_query(None), ViewMode::GapsOnly);
        assert_eq!(ViewMode::from_query(Some("xyz")), ViewMode::GapsOnly);
    }

    #[test]
    fn view_mode_round_trips_through_its_query_token() {
        for mode in [ViewMode::GapsOnly, ViewMode::All] {
            assert_eq!(ViewMode::from_query(Some(mode.as_query())), mode);
        }
    }

    #[test]
    fn view_mode_path_returns_canonical_url_per_mode() {
        assert_eq!(ViewMode::GapsOnly.path(), "/");
        assert_eq!(ViewMode::All.path(), "/?view=all");
    }

    #[tokio::test]
    async fn all_mode_builds_the_full_tree_including_covered_folders() {
        let dir = tempfile::tempdir().unwrap();
        // A gap and a covered book under the same author.
        touch(&dir.path().join("Author/Gap/01.mp3"));
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::All);
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
