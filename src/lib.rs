//! Internal scaffolding shared by the `missing-ebooks` and
//! `missing-ebooks-demo` binaries, the examples, the benches, and the
//! integration tests. This crate is binary-by-construction: every `pub`
//! item exists so a Cargo-built target in the same workspace can reach
//! it. There is no semver promise, no external consumer, and the
//! published artifact (`publish = false`) is the binary.
#![doc(hidden)]

pub mod autosync;
pub mod config;
pub mod demo;
pub mod marker;
pub mod query;
pub mod raw_view;
pub mod scanner;
pub mod scenarios;
pub mod shutdown;
pub mod state;
pub mod synthetic;
pub mod telemetry;
pub mod tree;
pub mod web;
