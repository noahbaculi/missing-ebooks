//! The public demo server: one process, in-memory per-session marks.
//!
//! Seeds a synthetic library into a temp directory, scans it into shared base
//! views, and serves the production UI with a demo banner. Each visitor is pinned
//! to an in-memory session by a cookie. Their marks are replayed on top of the
//! base view per request and never touch disk.

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;

use missing_ebooks::config::Config;
use missing_ebooks::demo::handlers::router as demo_router;
use missing_ebooks::demo::state::{DemoConfig, DemoState, build_state};
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::scenarios;

/// Demo CLI surface. Flags override the matching env vars. Everything else
/// continues to come from the environment (DEMO_MAX_SESSIONS, DEMO_IDLE_SECS,
/// DEMO_COOKIE_NAME).
#[derive(Parser, Debug)]
#[command(
    name = "missing-ebooks-demo",
    version,
    about = "Run the public-facing demo with a synthetic library.",
    after_help = "Environment variables:\n  \
        DEMO_BIND          IP:port to bind, e.g. 127.0.0.1:8080.\n  \
        DEMO_SCENARIO      Seeded scenario name, e.g. mixed-forest.\n  \
        DEMO_MAX_SESSIONS  Hard cap on concurrent sessions.\n  \
        DEMO_IDLE_SECS     Session idle window before the reaper drops it.\n  \
        DEMO_COOKIE_NAME   Session cookie name.\n\
        \nScenarios: mixed-forest, messy-shelf, clean-error, root-flagged, \
        pre-marked, big-library."
)]
struct Cli {
    /// Scenario name. Overrides DEMO_SCENARIO.
    #[arg(long)]
    scenario: Option<String>,
    /// Bind address (IP:port). Overrides DEMO_BIND.
    #[arg(long)]
    bind: Option<String>,
}

/// Read one variable, falling back to `default` when it is unset or empty.
fn var_or(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => default.to_string(),
    }
}

/// Build the demo config: defaults, then env-var overrides, then CLI overrides.
/// CLI flags win because they sit closest to the invocation.
fn load_config(cli: &Cli) -> Result<DemoConfig, Box<dyn std::error::Error>> {
    let bind = cli
        .bind
        .clone()
        .unwrap_or_else(|| var_or("DEMO_BIND", "127.0.0.1:8080"));
    let scenario = cli
        .scenario
        .clone()
        .unwrap_or_else(|| var_or("DEMO_SCENARIO", "mixed-forest"));
    Ok(DemoConfig {
        bind,
        scenario,
        max_sessions: var_or("DEMO_MAX_SESSIONS", "300").parse()?,
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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    missing_ebooks::telemetry::init();
    let cli = Cli::parse();
    let demo_config = load_config(&cli)?;

    // Resolve the scenario first, so an unknown name fails fast.
    let Some(scenario) = scenarios::find_scenario(&demo_config.scenario) else {
        eprintln!("Error: unknown scenario {:?}", demo_config.scenario);
        std::process::exit(1);
    };

    // Seed the scenario into a stable directory under /tmp. The data is synthetic
    // and the container is ephemeral, so it is never cleaned up explicitly. /tmp
    // matches the explore harness and keeps the root path short. It is a no-op in
    // the Linux container, where the platform temp dir is already /tmp.
    let seed_dir = std::path::Path::new("/tmp").join("missing-ebooks-demo");
    std::fs::create_dir_all(&seed_dir)?;
    let roots = scenarios::materialize(&(scenario.spec)(), &seed_dir);

    // The production config over the seeded roots, defaulted otherwise. The
    // demo builds a static base view once at startup (build_state) and never
    // rescans, so a polling client would just hit /refresh and get back the
    // same bytes. poll_interval_seconds=0 keeps the demo's zero-idle-work
    // property.
    let config = Config {
        library_roots: roots,
        poll_interval_seconds: 0,
        ..Default::default()
    };
    let settings = ScanSettings::compile(config.scan_inputs())?;

    let bind = demo_config.bind.clone();
    let state = Arc::new(build_state(config, settings, demo_config).await);

    tokio::spawn(run_reaper(state.clone()));

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "missing-ebooks demo listening");
    let serve = axum::serve(listener, demo_router(state))
        .with_graceful_shutdown(missing_ebooks::shutdown::signal());
    serve.await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn demo_help_flag_displays_help() {
        let err = Cli::try_parse_from(["missing-ebooks-demo", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn demo_version_flag_displays_version() {
        let err = Cli::try_parse_from(["missing-ebooks-demo", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn demo_scenario_and_bind_are_optional() {
        let cli = Cli::try_parse_from(["missing-ebooks-demo"]).unwrap();
        assert!(cli.scenario.is_none());
        assert!(cli.bind.is_none());
    }

    #[test]
    fn demo_flags_parse() {
        let cli = Cli::try_parse_from([
            "missing-ebooks-demo",
            "--scenario",
            "mixed-forest",
            "--bind",
            "0.0.0.0:9000",
        ])
        .unwrap();
        assert_eq!(cli.scenario.as_deref(), Some("mixed-forest"));
        assert_eq!(cli.bind.as_deref(), Some("0.0.0.0:9000"));
    }

    #[test]
    fn demo_after_help_lists_env_vars() {
        let help = Cli::command().render_help().to_string();
        for var in [
            "DEMO_BIND",
            "DEMO_SCENARIO",
            "DEMO_MAX_SESSIONS",
            "DEMO_IDLE_SECS",
            "DEMO_COOKIE_NAME",
        ] {
            assert!(help.contains(var), "{var} missing from demo --help");
        }
    }
}
