//! Web-agnostic service layer: pre-fold re-exports of items now in their final
//! homes (`web::render` for the read-view types and packaging helpers, `state`
//! for the domain error). The module exists only so `autosync` and `demo` keep
//! compiling on the path between Tasks 7-12 of the service-layer fold; it
//! disappears in Task 13 (see ADR-0028).

pub use crate::state::DomainError;
pub use crate::web::render::{FlaggedView, RootSection};

#[cfg(test)]
mod tests {}
