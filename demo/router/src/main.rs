//! Session router for the missing-ebooks public demo.
//!
//! Spawns one seeded `explore` sandbox per visitor, pins the browser to it with
//! a cookie, and reverse-proxies every later request to that process. See
//! docs/superpowers/specs/2026-06-08-demo-site-design.md. This binary is a thin
//! shell over the library in lib.rs, which the integration tests share.

use std::sync::Arc;

use tokio::sync::Mutex;

use missing_ebooks_demo_router::config::Config;
use missing_ebooks_demo_router::ports::PortPool;
use missing_ebooks_demo_router::proxy::{self, AppState, Inner};
use missing_ebooks_demo_router::sandbox::{self, RealLauncher};
use missing_ebooks_demo_router::session::SessionStore;
use missing_ebooks_demo_router::app;

/// The background reaper: every tick, sweep idle sandboxes through `reap_once`,
/// which kills their processes and returns their ports to the pool.
async fn run_reaper(state: Arc<AppState>) {
    let idle = state.config.idle;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        tick.tick().await;
        proxy::reap_once(&state, std::time::Instant::now(), idle).await;
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let config = Config::load_from_env()?;

    // Clear temp dirs left by a previous run before serving anything new.
    let swept = sandbox::sweep_temp_dirs(std::path::Path::new("/tmp"))?;
    if swept > 0 {
        tracing::info!(swept, "removed leftover explore temp dirs at startup");
    }

    let client = missing_ebooks_demo_router::proxy::http_client();
    let state = Arc::new(AppState {
        launcher: Box::new(RealLauncher {
            explore_bin: config.explore_bin.clone(),
            client: client.clone(),
        }),
        client,
        inner: Mutex::new(Inner {
            store: SessionStore::new(config.max_sandboxes, config.max_per_ip),
            pool: PortPool::new(config.port_low, config.port_high),
            children: Default::default(),
        }),
        config: config.clone(),
    });

    tokio::spawn(run_reaper(state.clone()));

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!(bind = %config.bind, "router listening");
    axum::serve(listener, app(state).into_make_service()).await?;
    Ok(())
}
