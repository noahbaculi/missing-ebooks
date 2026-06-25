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

    use crate::scanner::{RootScan, ScannedFolder};
    use std::path::PathBuf;

    #[test]
    fn audiobook_count_counts_walked_folders_that_directly_hold_audio() {
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
}
