//! Library face of the router so integration tests can build the same app the
//! binary serves. The binary in main.rs is a thin shell over these modules.

pub mod banner;
pub mod capacity;
pub mod config;
pub mod ports;
pub mod proxy;
pub mod sandbox;
pub mod session;

use std::sync::Arc;

use proxy::AppState;

/// Build the axum app over shared state.
pub fn app(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .fallback(proxy::handle)
        .with_state(state)
}
