//! In-memory per-session demo of the server. One process serves every visitor;
//! each visitor's marks live in memory keyed by a session cookie and never touch
//! disk. See
//! docs/superpowers/specs/2026-06-08-in-memory-demo-sandboxing-design.md.

pub mod session;
