//! Internal scaffolding shared by the `missing-ebooks` and
//! `missing-ebooks-demo` binaries, the benches, and the integration tests.
//! This crate is binary-by-construction: every `pub` item exists so a
//! Cargo-built target in the same workspace can reach it. There is no
//! semver promise, no external consumer, and the published artifact
//! (`publish = false`) is the binary.
// publish = false, so the crate's docs serve only in-workspace navigation. The
// CI `docs` job still builds them with -D warnings to keep doc-comments honest.
#![doc(hidden)]

pub mod config;
pub mod marker;
pub mod scanner;
pub mod web;

// The remaining modules have at least one in-workspace consumer (a binary,
// bench, or integration test) that reaches into them directly. They stay
// `pub` so the workspace targets keep compiling, but the crate-level
// `#![doc(hidden)]` above keeps them out of the rendered doc surface so
// they cannot be discovered as a public API.
pub mod demo;
pub mod scenarios;
pub mod shutdown;
pub mod state;
pub mod synthetic;
pub mod telemetry;
pub mod tree;

// Internal-only: no external consumer reaches into these.
pub(crate) mod autosync;
pub(crate) mod query;
