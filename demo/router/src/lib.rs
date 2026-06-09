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

/// Build the axum app over shared state. `/healthz` is a dedicated liveness
/// route so the container healthcheck does not flow through the sandbox-spawning
/// fallback; every other path is reverse-proxied to the visitor's sandbox.
pub fn app(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .route("/healthz", axum::routing::get(proxy::health))
        .fallback(proxy::handle)
        .with_state(state)
}
