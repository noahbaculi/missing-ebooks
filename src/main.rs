//! Server entry point: load config, build the shared state, and serve the
//! read-only web UI. `--print-config` still emits the template and exits.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use missing_ebooks::config::{CONFIG_TEMPLATE, Config, ConfigError};
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::state::AppState;
use missing_ebooks::web;

#[tokio::main]
async fn main() -> ExitCode {
    missing_ebooks::telemetry::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--print-config") {
        print!("{CONFIG_TEMPLATE}");
        return ExitCode::SUCCESS;
    }

    let config = match Config::load(parse_config_path(&args).as_deref()) {
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
            let _ = missing_ebooks::service::current_view(
                &state,
                missing_ebooks::tree::ViewMode::GapsOnly,
            )
            .await;
            tracing::debug!("startup cache warm complete");
        }
    });

    if let Err(err) = axum::serve(listener, app).await {
        tracing::error!(error = %err, "server error");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn parse_config_path(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            return iter.next().map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_path_in_both_forms() {
        assert_eq!(
            parse_config_path(&["--config".to_string(), "/a/b.toml".to_string()]),
            Some(PathBuf::from("/a/b.toml"))
        );
        assert_eq!(
            parse_config_path(&["--config=/c/d.toml".to_string()]),
            Some(PathBuf::from("/c/d.toml"))
        );
        assert_eq!(parse_config_path(&["--print-config".to_string()]), None);
    }
}
