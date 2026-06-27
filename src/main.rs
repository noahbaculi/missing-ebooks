//! Server entry point: load config, build shared state, serve the web UI.
//! `--print-config` emits the config template and exits.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;

use missing_ebooks::config::{CONFIG_TEMPLATE, Config, ConfigError};
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::state::AppState;
use missing_ebooks::web;

/// Command-line surface. Environment variables remain the primary
/// configuration path. Flags layer on top per `Config::load`'s precedence.
#[derive(Parser, Debug)]
#[command(
    name = "missing-ebooks",
    version,
    about = "Surface audiobook folders that hold audio but no matching ebook.",
    after_help = "Environment variables:\n  \
        MISSING_EBOOKS_LIBRARY_ROOTS  Colon-separated paths to scan.\n  \
        MISSING_EBOOKS_BIND           IP to bind, e.g. 127.0.0.1.\n  \
        MISSING_EBOOKS_PORT           TCP port, e.g. 8080.\n  \
        MISSING_EBOOKS_CONFIG         Optional config file path (same as --config).\n  \
        MISSING_EBOOKS_LOG            Tracing filter, e.g. info,missing_ebooks=debug.\n\
        \nSee README for the full env-var list."
)]
struct Cli {
    /// Print the bundled configuration template as TOML and exit.
    #[arg(long)]
    print_config: bool,

    /// Path to a configuration file. Defaults to MISSING_EBOOKS_CONFIG or none.
    #[arg(long, value_name = "PATH", env = "MISSING_EBOOKS_CONFIG")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> ExitCode {
    missing_ebooks::telemetry::init();

    let cli = Cli::parse();

    if cli.print_config {
        print!("{CONFIG_TEMPLATE}");
        return ExitCode::SUCCESS;
    }

    let config = match Config::load(cli.config.as_deref()) {
        Ok(cfg) => cfg,
        Err(err @ ConfigError::MissingLibraryRoots) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(1);
        }
    };

    let settings = match ScanSettings::compile(config.scan_inputs()) {
        Ok(settings) => settings,
        Err(err) => {
            tracing::error!(error = %err, "invalid scan settings");
            return ExitCode::from(1);
        }
    };

    let ip: IpAddr = match config.bind.parse() {
        Ok(ip) => ip,
        Err(_) => {
            tracing::error!(bind = %config.bind, "bind is not a valid IP address");
            return ExitCode::from(1);
        }
    };
    if !ip.is_loopback() {
        tracing::warn!(
            bind = %config.bind,
            "binding to a non-loopback address; the server has no authentication"
        );
    }
    let addr = SocketAddr::new(ip, config.port);

    // Size the scan thread pool by the configured concurrency, not the core
    // count: the directory walk is bound by network round-trip latency, so the
    // threads mostly wait on the wire and stay useful well above the CPU count
    // (and survive a container CPU limit). build_global is called once per
    // process; a failure leaves rayon's default pool in place.
    if let Err(err) = rayon::ThreadPoolBuilder::new()
        .num_threads(config.scan_concurrency.max(1))
        .build_global()
    {
        tracing::warn!(error = %err, "could not size the scan thread pool; using rayon defaults");
    }

    let state = Arc::new(AppState::new(config, settings));
    let app = web::router(Arc::clone(&state));

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!(%addr, error = %err, "could not bind the listener");
            return ExitCode::from(1);
        }
    };
    tracing::info!(url = %format!("http://{addr}"), "missing-ebooks listening");

    // Warm the default (gaps-only) view in the background so the first viewer
    // after a restart does not pay the cold scan, which is slow over a network
    // mount. The server starts serving immediately; a request that arrives
    // before the warm finishes single-flights on the same cache lock, so this
    // never double-scans. The show-all slot stays lazy until first asked.
    tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            // Warm the gaps-mode slot. The packaging is cheap; the cache
            // slot side effect is what we want.
            state.warm().await;
            tracing::debug!("startup cache warm complete");
        }
    });

    let serve =
        axum::serve(listener, app).with_graceful_shutdown(missing_ebooks::shutdown::signal());
    if let Err(err) = serve.await {
        tracing::error!(error = %err, "server error");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn help_flag_displays_help() {
        let err = Cli::try_parse_from(["missing-ebooks", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn version_flag_displays_version() {
        let err = Cli::try_parse_from(["missing-ebooks", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn print_config_is_a_bool_flag() {
        let cli = Cli::try_parse_from(["missing-ebooks", "--print-config"]).unwrap();
        assert!(cli.print_config);
        assert!(cli.config.is_none());
    }

    #[test]
    fn config_path_accepts_both_forms() {
        let cli = Cli::try_parse_from(["missing-ebooks", "--config", "/a/b.toml"]).unwrap();
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/a/b.toml"))
        );
        let cli = Cli::try_parse_from(["missing-ebooks", "--config=/c/d.toml"]).unwrap();
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/c/d.toml"))
        );
    }

    #[test]
    fn after_help_lists_env_vars() {
        // Pins the env-var enumeration so a future change that adds a var here
        // does not silently leave the after_help out of date.
        let help = Cli::command().render_help().to_string();
        for var in [
            "MISSING_EBOOKS_LIBRARY_ROOTS",
            "MISSING_EBOOKS_BIND",
            "MISSING_EBOOKS_PORT",
            "MISSING_EBOOKS_CONFIG",
            "MISSING_EBOOKS_LOG",
        ] {
            assert!(help.contains(var), "{var} missing from --help");
        }
    }
}
