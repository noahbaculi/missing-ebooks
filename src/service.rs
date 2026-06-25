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
mod tests {}
