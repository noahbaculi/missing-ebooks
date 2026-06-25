//! Web-agnostic service layer: pre-fold re-exports of items now in their final
//! homes (`web::render` for the read-view types and packaging helpers, `state`
//! for the domain error). The module exists only so `autosync` and `demo` keep
//! compiling on the path between Tasks 7-12 of the service-layer fold; it
//! disappears in Task 13 (see ADR-0028).

pub use crate::state::DomainError;
pub use crate::web::render::{FlaggedView, RootSection};
// Pre-fold helper names. The new homes are package_view and package_section in
// web::render. These re-exports go away with service.rs itself.
pub(crate) use crate::web::render::package_section as render_section_from_raw;
pub(crate) use crate::web::render::package_view as render_view;

#[cfg(test)]
mod tests {}
