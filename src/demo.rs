//! In-memory per-session demo of the server. One process serves every visitor;
//! each visitor's marks live in memory keyed by a session cookie and never touch
//! disk. See
//! docs/superpowers/specs/2026-06-08-in-memory-demo-sandboxing-design.md.

pub mod banner;
pub mod handlers;
pub mod session;
pub mod state;

pub use handlers::router;
pub use state::{DemoConfig, DemoState, build_state};
