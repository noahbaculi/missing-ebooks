//! Server entry point: load config, build shared state, serve the web UI.
//! `--print-config` emits the config template and exits.

use std::ffi::OsString;
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
        MISSING_EBOOKS_CONFIG         Config file path; ignored if no file exists there.\n  \
        MISSING_EBOOKS_LOG            Tracing filter, e.g. info,missing_ebooks=debug.\n\
        \nSee README for the full env-var list."
)]
struct Cli {
    /// Print the bundled configuration template as TOML and exit.
    #[arg(long)]
    print_config: bool,

    /// Path to a configuration file. Defaults to MISSING_EBOOKS_CONFIG or none.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

/// Resolve the config path: an explicit flag wins, an env-provided path only counts if it exists
fn resolve_config_path(flag: Option<PathBuf>, env: Option<OsString>) -> Option<PathBuf> {
    flag.or_else(|| env.map(PathBuf::from).filter(|path| path.is_file()))
}

/// Parsed value of `MISSING_EBOOKS_ALLOW_PUBLIC_BIND`.
#[derive(Debug)]
enum AllowPublicBind {
    On,
    Off,
    Invalid(String),
}

/// Parses `MISSING_EBOOKS_ALLOW_PUBLIC_BIND`.
///
/// Accepted truthy values are `1`, `true`, `yes`, `on`, case-insensitive
/// and whitespace-trimmed. Unset or empty is `Off`. Anything else is
/// `Invalid` carrying the trimmed original.
fn parse_allow_public_bind(raw: Option<&str>) -> AllowPublicBind {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => AllowPublicBind::Off,
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => AllowPublicBind::On,
            _ => AllowPublicBind::Invalid(s.to_string()),
        },
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    missing_ebooks::telemetry::init();

    let cli = Cli::parse();

    if cli.print_config {
        print!("{CONFIG_TEMPLATE}");
        return ExitCode::SUCCESS;
    }

    let config_path = resolve_config_path(cli.config, std::env::var_os("MISSING_EBOOKS_CONFIG"));
    let config = match Config::load(config_path.as_deref()) {
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
        let allow_env = std::env::var("MISSING_EBOOKS_ALLOW_PUBLIC_BIND").ok();
        match parse_allow_public_bind(allow_env.as_deref()) {
            AllowPublicBind::On => {
                tracing::warn!(
                    bind = %config.bind,
                    "binding to a non-loopback address with MISSING_EBOOKS_ALLOW_PUBLIC_BIND opted in (accepted values: 1, true, yes, on). Put a reverse proxy with authentication in front before exposing this."
                );
            }
            AllowPublicBind::Off => {
                tracing::error!(
                    bind = %config.bind,
                    "refusing to bind a non-loopback address. Set MISSING_EBOOKS_ALLOW_PUBLIC_BIND to opt in (accepted values, case-insensitive: 1, true, yes, on) after reading SECURITY.md, or bind loopback and front with a reverse proxy that enforces auth."
                );
                return ExitCode::from(1);
            }
            AllowPublicBind::Invalid(value) => {
                tracing::error!(
                    bind = %config.bind,
                    value = %value,
                    "MISSING_EBOOKS_ALLOW_PUBLIC_BIND has an unrecognized value; accepted values (case-insensitive): 1, true, yes, on"
                );
                return ExitCode::from(1);
            }
        }
    }
    let addr = SocketAddr::new(ip, config.port);

    // Size the scan thread pool by the configured concurrency, not the core
    // count: the directory walk is bound by network round-trip latency, so the
    // threads mostly wait on the wire and stay useful well above the CPU count
    // (and survive a container CPU limit). build_global is called once per
    // process. A failure leaves rayon's default pool in place.
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
    // mount. The server starts serving immediately. A request that arrives
    // before the warm finishes single-flights on the same cache lock, so this
    // never double-scans. The show-all slot stays lazy until first asked.
    tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            // Warm the gaps-mode slot. The packaging is cheap. The cache
            // slot side effect is the point.
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
    use std::ffi::OsString;

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

    #[test]
    fn env_config_path_that_does_not_exist_resolves_to_none() {
        let resolved = resolve_config_path(None, Some(OsString::from("/definitely/not/here.toml")));
        assert_eq!(resolved, None);
    }

    #[test]
    fn env_config_path_that_exists_resolves_to_some() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();

        let resolved = resolve_config_path(None, Some(path.clone().into_os_string()));
        assert_eq!(resolved, Some(path));
    }

    #[test]
    fn explicit_flag_wins_over_env_even_when_the_file_is_absent() {
        // A typed path is a promise: only the env hint may vanish
        let flag = PathBuf::from("/typed/but/missing.toml");
        let resolved = resolve_config_path(
            Some(flag.clone()),
            Some(OsString::from("/definitely/not/here.toml")),
        );
        assert_eq!(resolved, Some(flag));
    }

    #[test]
    fn unset_is_off() {
        assert!(matches!(
            parse_allow_public_bind(None),
            AllowPublicBind::Off
        ));
    }

    #[test]
    fn empty_and_whitespace_are_off() {
        assert!(matches!(
            parse_allow_public_bind(Some("")),
            AllowPublicBind::Off
        ));
        assert!(matches!(
            parse_allow_public_bind(Some("   ")),
            AllowPublicBind::Off
        ));
    }

    #[test]
    fn canonical_truthy_values_are_on() {
        for raw in ["1", "true", "yes", "on"] {
            assert!(
                matches!(parse_allow_public_bind(Some(raw)), AllowPublicBind::On),
                "{raw} should parse as On"
            );
        }
    }

    #[test]
    fn truthy_values_are_case_insensitive_and_trimmed() {
        for raw in ["TRUE", "Yes", "  on  ", "\tON\n"] {
            assert!(
                matches!(parse_allow_public_bind(Some(raw)), AllowPublicBind::On),
                "{raw} should parse as On"
            );
        }
    }

    #[test]
    fn unknown_values_are_invalid_and_carry_the_trimmed_original() {
        for raw in ["0", "false", "no", "off", "nope"] {
            match parse_allow_public_bind(Some(raw)) {
                AllowPublicBind::Invalid(v) => assert_eq!(v, raw.trim()),
                other => panic!("{raw} should be Invalid, got {other:?}"),
            }
        }
    }

    #[test]
    fn invalid_preserves_case_of_the_offending_input() {
        match parse_allow_public_bind(Some("  Nope  ")) {
            AllowPublicBind::Invalid(v) => assert_eq!(v, "Nope"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}
