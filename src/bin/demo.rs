//! The public demo server: one process, in-memory per-session marks.
//!
//! Seeds a synthetic library into a temp directory, scans it into shared base
//! views, and serves the production UI with a demo banner. Each visitor is pinned
//! to an in-memory session by a cookie. Their marks are replayed on top of the
//! base view per request and never touch disk.

use std::sync::Arc;
use std::time::{Duration, Instant};

use missing_ebooks::config::Config;
use missing_ebooks::demo::handlers::router as demo_router;
use missing_ebooks::demo::state::{DemoConfig, DemoState, build_state};
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::scenarios;

/// Read one variable, falling back to `default` when it is unset or empty.
fn var_or(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => default.to_string(),
    }
}

/// Build the demo config from the environment, applying defaults for anything
/// unset. The scenario default matches the former router default.
fn load_config() -> anyhow::Result<DemoConfig> {
    Ok(DemoConfig {
        bind: var_or("DEMO_BIND", "127.0.0.1:8080"),
        scenario: var_or("DEMO_SCENARIO", "mixed-forest"),
        max_sessions: var_or("DEMO_MAX_SESSIONS", "1000").parse()?,
        idle: Duration::from_secs(var_or("DEMO_IDLE_SECS", "1200").parse()?),
        cookie_name: var_or("DEMO_COOKIE_NAME", "me_demo_sid"),
    })
}

/// Sweep idle sessions on a fixed tick.
async fn run_reaper(state: Arc<DemoState>) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        let reaped = state.reap_idle(Instant::now());
        if reaped > 0 {
            tracing::info!(reaped, "dropped idle demo sessions");
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    missing_ebooks::telemetry::init();
    let demo_config = load_config()?;

    // Resolve the scenario first, so an unknown name fails fast.
    let scenario = scenarios::find_scenario(&demo_config.scenario)
        .ok_or_else(|| anyhow::anyhow!("unknown scenario {:?}", demo_config.scenario))?;

    // Seed the scenario into a stable directory under /tmp. The data is synthetic
    // and the container is ephemeral, so it is never cleaned up explicitly. /tmp
    // matches the explore harness and keeps the root path short. It is a no-op in
    // the Linux container, where the platform temp dir is already /tmp.
    let seed_dir = std::path::Path::new("/tmp").join("missing-ebooks-demo");
    std::fs::create_dir_all(&seed_dir)?;
    let roots = (scenario.build)(&seed_dir);

    // The production config over the seeded roots, defaulted otherwise.
    // autosync_interval_seconds=0 disables the autosync loop everywhere a
    // production AppState would build one (ADR-0023). The demo never builds an
    // AppState today, so this is a placeholder that documents the choice: the
    // session sweep's idle signal does not yet track SSE traffic, so per-session
    // loops would extend sessions inappropriately. A follow-up captures
    // showcasing autosync in the demo properly.
    let config = Config {
        library_roots: roots,
        autosync_interval_seconds: 0,
        ..Default::default()
    };
    let settings = ScanSettings::compile(config.scan_inputs())?;

    let bind = demo_config.bind.clone();
    let state = Arc::new(build_state(config, settings, demo_config).await);

    tokio::spawn(run_reaper(state.clone()));

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "missing-ebooks demo listening");
    let serve = axum::serve(listener, demo_router(state)).with_graceful_shutdown(async {
        tokio::signal::ctrl_c().await.ok();
    });
    serve.await?;
    Ok(())
}
