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
use missing_ebooks_demo_router::proxy::{AppState, Inner};
use missing_ebooks_demo_router::sandbox::{self, RealLauncher};
use missing_ebooks_demo_router::session::SessionStore;
use missing_ebooks_demo_router::app;

/// The background reaper: every tick, remove idle sandboxes, SIGINT their
/// processes, and return their ports to the pool.
async fn run_reaper(state: Arc<AppState>) {
    let idle = state.config.idle;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        tick.tick().await;
        let now = std::time::Instant::now();
        let (reaped, mut children) = {
            let mut inner = state.inner.lock().await;
            let reaped = inner.store.reap_idle(now, idle);
            let mut children = Vec::new();
            for s in &reaped {
                inner.pool.release(s.port);
                if let Some(child) = inner.children.remove(&s.pid) {
                    children.push(child);
                }
            }
            (reaped, children)
        };
        for s in &reaped {
            // SIGINT lets explore remove its temp dir before exiting.
            sandbox::shutdown(s.pid);
            tracing::info!(port = s.port, pid = s.pid, "reaped idle sandbox");
        }
        // Wait the exited children so they do not linger as zombies under PID 1.
        for mut child in children.drain(..) {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
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
