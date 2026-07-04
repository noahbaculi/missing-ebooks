//! Internal scaffolding shared by the `missing-ebooks` and
//! `missing-ebooks-demo` binaries, the benches, and the integration tests.
//! This crate is binary-by-construction: every `pub` item exists so a
//! Cargo-built target in the same workspace can reach it. There is no
//! semver promise, no external consumer, and the published artifact
//! (`publish = false`) is the binary.

pub mod config;
pub mod marker;
pub mod scanner;
pub mod web;

// The remaining modules have at least one in-workspace consumer (a binary,
// bench, or integration test) that reaches into them directly. They stay
// `pub` so the workspace targets keep compiling. The crate is `publish =
// false`, so the rendered docs only serve in-workspace navigation.
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
