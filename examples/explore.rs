//! Disposable, seeded instance of the real server for eyeballing the UI.
//!
//! `cargo run --example explore -- <scenario>` seeds a synthetic library into a
//! temp directory, serves it through the production `web::router` and `AppState`
//! unchanged, and tears the directory down on Ctrl-C. There are no assertions and
//! no browser automation: it just lets you click around a catalog of known
//! library states. Loose root audio surfaces the root itself (see
//! docs/adr/0007-folder-granular-not-book-granular.md).

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr};
use std::process::ExitCode;
use std::sync::Arc;

use axum::http::{HeaderValue, header};
use axum::response::Response;
use clap::Parser;
use missing_ebooks::config::Config;
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::scenarios;
use missing_ebooks::state::AppState;
use missing_ebooks::web;
use tokio::net::TcpListener;

/// Explore-harness CLI surface. Mirrors `bin/demo.rs` and `main.rs`: a clap
/// derive struct with the same flag set the hand-rolled parser carried
/// (positional scenario, `--port`, `--ttl`, `--keep`, `--help`, `--version`).
#[derive(clap::Parser, Debug)]
#[command(
    name = "explore",
    version,
    about = "Serve the production UI against a seeded synthetic library for eyeballing.",
    after_help = "Scenarios: mixed-forest, messy-shelf, clean-error, root-flagged, \
        pre-marked, big-library.\n\nRun with no scenario to print the catalog and exit."
)]
struct Cli {
    /// Scenario name (one of: mixed-forest, messy-shelf, clean-error, root-flagged, pre-marked, big-library).
    scenario: Option<String>,
    /// Bind an exact port instead of the default 13379.
    #[arg(long)]
    port: Option<u16>,
    /// Scan-cache staleness window in seconds (default 0, cache off).
    #[arg(long = "scan-cache-ttl", default_value_t = 0)]
    scan_cache_ttl_seconds: u64,
    /// Keep the seeded files on exit and print where they landed.
    #[arg(long)]
    keep: bool,
    /// Bind a specific local IP. Defaults to 127.0.0.1. Non-loopback binds skip
    /// the preferred-port fallback and warn on stderr.
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    bind: IpAddr,
}

/// Every scenario name and its description. Printed to stderr when a missing or
/// unknown scenario name leaves nothing to serve. Clap renders its own help.
fn catalog_listing() -> String {
    let mut out = String::from("scenarios:\n");
    for scenario in scenarios::catalog() {
        // Helper output, infallible write into a String.
        let _ = writeln!(out, "  {:<14}{}", scenario.name, scenario.description);
    }
    out
}

/// Bind the harness listener. On loopback, keep the ADR-0011 preferred-port
/// fallback: with no explicit --port, prefer `default_port` and fall back to an
/// OS-assigned port only if it is taken. On any non-loopback bind (tailnet IP,
/// 0.0.0.0), stay exact-or-error even without --port: a random high port on an
/// exposed interface is a worse default than a clear failure. An explicit --port
/// is always exact, regardless of bind.
async fn bind_harness_listener(
    bind: IpAddr,
    explicit: Option<u16>,
    default_port: u16,
) -> std::io::Result<TcpListener> {
    let preferred = explicit.unwrap_or(default_port);
    match TcpListener::bind((bind, preferred)).await {
        Ok(listener) => Ok(listener),
        Err(err)
            if explicit.is_none()
                && bind.is_loopback()
                && err.kind() == std::io::ErrorKind::AddrInUse =>
        {
            eprintln!("port {preferred} is in use; serving on an OS-assigned port instead");
            TcpListener::bind((bind, 0)).await
        }
        Err(err) => Err(err),
    }
}

/// Stamp a harness response `no-store` so the browser never caches it. Layered
/// over the whole router in dev, it overrides the production `Cache-Control` on
/// every response (asset or page) so each reload fetches the freshly rebuilt
/// bytes, with no hard reload or port change.
async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // A missing or unknown scenario prints the catalog to stderr and exits
    // non-zero, so a typo lands you on the menu rather than a blank server.
    let Some(scenario) = cli.scenario.as_deref().and_then(scenarios::find_scenario) else {
        eprint!("{}", catalog_listing());
        return ExitCode::from(2);
    };

    // Initialize tracing only once we are committed to serving, so the warnings
    // the scanner emits (an unreadable root, for example) are visible.
    missing_ebooks::telemetry::init();

    let temp = match tempfile::Builder::new()
        .prefix("explore-")
        .tempdir_in("/tmp")
    {
        Ok(temp) => temp,
        Err(err) => {
            eprintln!("could not create a temp directory under /tmp: {err}");
            return ExitCode::FAILURE;
        }
    };
    let roots = scenarios::materialize(&(scenario.spec)(), &temp.path().join(scenario.name));

    // The real server config, defaulted except for the seeded roots and the TTL.
    // The default search links stay so the row links render. The example binds
    // its own listener below, preferring the app's default port.
    let config = Config {
        library_roots: roots,
        scan_cache_ttl_seconds: cli.scan_cache_ttl_seconds,
        ..Default::default()
    };
    let settings = match ScanSettings::compile(config.scan_inputs()) {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("invalid scan settings: {err}");
            return ExitCode::FAILURE;
        }
    };
    let state = Arc::new(AppState::new(config, settings));
    // Dev convenience only: the production asset handlers cache the stylesheet and
    // script for an hour, so after an edit-and-rebuild a browser serves the stale
    // copy until a hard reload or a fresh port. This harness exists to eyeball live
    // edits, so override every response to `no-store`. The production server keeps
    // its real cache policy (see src/web/assets.rs).
    let app = web::router(state).layer(axum::middleware::map_response(no_store));

    let listener = match bind_harness_listener(cli.bind, cli.port, Config::default().port).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("could not bind {}: {err}", cli.bind);
            return ExitCode::FAILURE;
        }
    };
    let addr = listener
        .local_addr()
        .expect("a bound listener has a local address");
    if !cli.bind.is_loopback() {
        eprintln!("warning: binding {addr} exposes the harness beyond localhost");
    }
    println!("scenario: {}", scenario.name);
    println!("listening on http://{addr}");
    println!("Press Ctrl-C to stop.");

    let serve = axum::serve(listener, app).with_graceful_shutdown(async {
        tokio::signal::ctrl_c().await.ok();
    });
    if let Err(err) = serve.await {
        eprintln!("server error: {err}");
        return ExitCode::FAILURE;
    }

    // On exit, keep the seeded files when asked (and print where they landed),
    // otherwise let the TempDir drop and remove them.
    if cli.keep {
        let kept = temp.keep();
        println!("kept the seeded library at {}", kept.display());
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_listing_carries_every_scenario_name() {
        let listing = catalog_listing();
        for name in [
            "mixed-forest",
            "messy-shelf",
            "clean-error",
            "root-flagged",
            "pre-marked",
            "big-library",
        ] {
            assert!(listing.contains(name), "listing is missing {name}");
        }
    }

    #[test]
    fn cli_parses_bare_scenario_with_defaults() {
        let cli = Cli::try_parse_from(["explore", "mixed-forest"]).unwrap();
        assert_eq!(cli.scenario.as_deref(), Some("mixed-forest"));
        assert_eq!(cli.port, None);
        assert_eq!(cli.scan_cache_ttl_seconds, 0);
        assert!(!cli.keep);
    }

    #[test]
    fn cli_parses_every_flag() {
        let cli = Cli::try_parse_from([
            "explore",
            "clean-error",
            "--port",
            "9000",
            "--scan-cache-ttl",
            "30",
            "--keep",
        ])
        .unwrap();
        assert_eq!(cli.scenario.as_deref(), Some("clean-error"));
        assert_eq!(cli.port, Some(9000));
        assert_eq!(cli.scan_cache_ttl_seconds, 30);
        assert!(cli.keep);
    }

    #[test]
    fn cli_help_and_version_exit() {
        let help_err = Cli::try_parse_from(["explore", "--help"]).unwrap_err();
        assert_eq!(help_err.kind(), clap::error::ErrorKind::DisplayHelp);
        let version_err = Cli::try_parse_from(["explore", "--version"]).unwrap_err();
        assert_eq!(version_err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[tokio::test]
    async fn binds_the_preferred_port_when_it_is_free() {
        // Reserve a port to learn a free number, release it, then confirm the
        // harness binds exactly that preferred port rather than moving off it.
        let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let free = probe.local_addr().unwrap().port();
        drop(probe);

        let listener = bind_harness_listener(Ipv4Addr::LOCALHOST.into(), None, free)
            .await
            .unwrap();
        assert_eq!(listener.local_addr().unwrap().port(), free);
    }

    #[tokio::test]
    async fn falls_back_to_an_ephemeral_port_when_the_preferred_one_is_taken() {
        // Hold the preferred port, then ask the harness to prefer it. With no
        // explicit --port it binds a different OS-assigned port instead of
        // failing.
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let taken = held.local_addr().unwrap().port();

        let listener = bind_harness_listener(Ipv4Addr::LOCALHOST.into(), None, taken)
            .await
            .unwrap();
        assert_ne!(listener.local_addr().unwrap().port(), taken);
    }

    #[tokio::test]
    async fn an_explicit_port_conflict_is_surfaced_as_an_error() {
        // An explicit --port is exact: a conflict must error, not silently move
        // to another port the way the defaulting path does.
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let taken = held.local_addr().unwrap().port();

        assert!(
            bind_harness_listener(Ipv4Addr::LOCALHOST.into(), Some(taken), 0)
                .await
                .is_err()
        );
    }

    #[test]
    fn cli_defaults_bind_to_loopback() {
        let cli = Cli::try_parse_from(["explore", "mixed-forest"]).unwrap();
        assert_eq!(cli.bind, std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn cli_parses_bind_flag() {
        let cli = Cli::try_parse_from(["explore", "mixed-forest", "--bind", "0.0.0.0"]).unwrap();
        assert_eq!(cli.bind, std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[tokio::test]
    async fn loopback_fallback_still_works_when_bind_is_explicit_loopback() {
        // Regression guard: passing --bind 127.0.0.1 explicitly must not disable
        // the ADR-0011 preferred-port fallback.
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let taken = held.local_addr().unwrap().port();

        let listener = bind_harness_listener(Ipv4Addr::LOCALHOST.into(), None, taken)
            .await
            .unwrap();
        assert_ne!(listener.local_addr().unwrap().port(), taken);
    }

    #[tokio::test]
    async fn non_loopback_bind_errors_on_port_conflict() {
        // Non-loopback binds are exact-or-error even with no --port, so a random
        // high port on an exposed interface never surprises a user.
        let held = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await.unwrap();
        let taken = held.local_addr().unwrap().port();

        assert!(
            bind_harness_listener(Ipv4Addr::UNSPECIFIED.into(), None, taken)
                .await
                .is_err()
        );
    }
}
