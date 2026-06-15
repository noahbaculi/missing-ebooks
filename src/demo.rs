//! In-memory per-session demo of the server. One process serves every visitor.
//! Each visitor's marks live in memory keyed by a session cookie and never touch
//! disk.

pub mod banner;
pub mod handlers;
pub mod session;
pub mod state;

pub use handlers::router;
pub use state::{DemoConfig, DemoState, build_state};
