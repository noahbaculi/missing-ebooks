//! Disposable, seeded instance of the real server for eyeballing the UI.
//!
//! `cargo run --example explore -- <scenario>` seeds a synthetic library into a
//! temp directory, serves it through the production `web::router` and `AppState`
//! unchanged, and tears the directory down on Ctrl-C. There are no assertions and
//! no browser automation: it just lets you click around a catalog of known
//! library states. This is the repeatable UI harness listed under the README's
//! "Future work". Loose root audio surfaces the root itself (see
//! docs/adr/0005-library-root-itself-flaggable.md).

use std::net::Ipv4Addr;
use std::process::ExitCode;
use std::sync::Arc;

use axum::http::{HeaderValue, header};
use axum::response::Response;
use missing_ebooks::config::Config;
use missing_ebooks::scanner::ScanSettings;
use missing_ebooks::scenarios;
use missing_ebooks::state::AppState;
use missing_ebooks::web;
use tokio::net::TcpListener;

/// The usage line, printed beside the scenario catalog on a bad or absent name.
const USAGE: &str =
    "usage: cargo run --example explore -- <scenario> [--port N] [--ttl SECS] [--keep]";

/// A parsed command line. `scenario` is `None` when no positional name was given,
/// which the catalog lookup treats the same as an unknown name.
#[derive(Debug, PartialEq)]
struct Args {
    scenario: Option<String>,
    port: Option<u16>,
    ttl_seconds: u64,
    keep: bool,
}

/// Parse the argument vector (already stripped of the program name). `Ok(None)`
/// means help was requested; `Ok(Some(args))` is a run request; `Err(message)` is
/// a usage error the caller prints beside the catalog. Hand-rolled to match the
/// `--config` / `--print-config` handling in `main.rs`; no clap.
fn parse_args(argv: &[String]) -> Result<Option<Args>, String> {
    let mut scenario = None;
    let mut port = None;
    let mut ttl_seconds = 0u64;
    let mut keep = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg == "--keep" {
            keep = true;
        } else if arg == "--port" {
            let value = iter
                .next()
                .ok_or_else(|| "--port needs a value".to_string())?;
            port = Some(parse_port(value)?);
        } else if let Some(value) = arg.strip_prefix("--port=") {
            port = Some(parse_port(value)?);
        } else if arg == "--ttl" {
            let value = iter
                .next()
                .ok_or_else(|| "--ttl needs a value".to_string())?;
            ttl_seconds = parse_ttl(value)?;
        } else if let Some(value) = arg.strip_prefix("--ttl=") {
            ttl_seconds = parse_ttl(value)?;
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag {arg:?}"));
        } else if scenario.is_some() {
            return Err(format!("unexpected extra argument {arg:?}"));
        } else {
            scenario = Some(arg.clone());
        }
    }
    Ok(Some(Args {
        scenario,
        port,
        ttl_seconds,
        keep,
    }))
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse()
        .map_err(|_| format!("--port: {value:?} is not a valid port number"))
}

fn parse_ttl(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("--ttl: {value:?} is not a valid number of seconds"))
}

/// The usage line followed by every scenario name and its description. Printed to
/// stdout for `--help` and to stderr for a missing or unknown scenario name.
fn catalog_listing() -> String {
    let mut out = String::new();
    out.push_str(USAGE);
    out.push_str("\n\nscenarios:\n");
    for scenario in scenarios::catalog() {
        out.push_str(&format!(
            "  {:<14}{}\n",
            scenario.name, scenario.description
        ));
    }
    out
}

/// Bind the harness's loopback listener. With no explicit `--port`, prefer
/// `default_port` (the application's own default) so the printed URL matches a
/// real deployment, and fall back to an OS-assigned port only when that port is
/// already taken. An explicit port is bound exactly, so a conflict there is a
/// real error the caller surfaces rather than papering over.
async fn bind_harness_listener(
    explicit: Option<u16>,
    default_port: u16,
) -> std::io::Result<TcpListener> {
    let preferred = explicit.unwrap_or(default_port);
    match TcpListener::bind((Ipv4Addr::LOCALHOST, preferred)).await {
        Ok(listener) => Ok(listener),
        // Only the defaulting path falls back; an explicit --port stays exact,
        // so its conflict propagates to the caller.
        Err(err) if explicit.is_none() && err.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("port {preferred} is in use; serving on an OS-assigned port instead");
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await
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
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(Some(args)) => args,
        Ok(None) => {
            // --help / -h: the listing goes to stdout and we exit cleanly.
            print!("{}", catalog_listing());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("error: {message}\n");
            eprint!("{}", catalog_listing());
            return ExitCode::from(2);
        }
    };

    // A missing or unknown scenario prints the catalog to stderr and exits
    // non-zero, so a typo lands you on the menu rather than a blank server.
    let scenario = match args.scenario.as_deref().and_then(scenarios::find_scenario) {
        Some(scenario) => scenario,
        None => {
            eprint!("{}", catalog_listing());
            return ExitCode::from(2);
        }
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
    let roots = (scenario.build)(&temp.path().join(scenario.name));

    // The real server config, defaulted except for the seeded roots and the TTL.
    // The default search links stay so the row links render. The example binds
    // its own listener below, preferring the app's default port.
    let config = Config {
        library_roots: roots,
        ttl_seconds: args.ttl_seconds,
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

    // Bind 127.0.0.1. With no --port, prefer the app's default so the printed
    // URL matches a real deployment, falling back to an OS-assigned port only if
    // the default is already taken. --port pins an exact port for a stable URL.
    let listener = match bind_harness_listener(args.port, Config::default().port).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("could not bind 127.0.0.1: {err}");
            return ExitCode::FAILURE;
        }
    };
    let addr = listener
        .local_addr()
        .expect("a bound listener has a local address");
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
    if args.keep {
        let kept = temp.keep();
        println!("kept the seeded library at {}", kept.display());
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_a_bare_scenario_name_with_defaults() {
        assert_eq!(
            parse_args(&argv(&["mixed-forest"])),
            Ok(Some(Args {
                scenario: Some("mixed-forest".to_string()),
                port: None,
                ttl_seconds: 0,
                keep: false,
            }))
        );
    }

    #[test]
    fn parses_every_flag_in_space_form() {
        assert_eq!(
            parse_args(&argv(&[
                "clean-error",
                "--port",
                "9000",
                "--ttl",
                "30",
                "--keep"
            ])),
            Ok(Some(Args {
                scenario: Some("clean-error".to_string()),
                port: Some(9000),
                ttl_seconds: 30,
                keep: true,
            }))
        );
    }

    #[test]
    fn parses_port_and_ttl_in_equals_form() {
        let parsed = parse_args(&argv(&["root-flagged", "--port=8081", "--ttl=5"])).unwrap();
        let args = parsed.unwrap();
        assert_eq!(args.port, Some(8081));
        assert_eq!(args.ttl_seconds, 5);
    }

    #[test]
    fn help_short_circuits_to_none() {
        assert_eq!(parse_args(&argv(&["--help"])), Ok(None));
        assert_eq!(parse_args(&argv(&["-h"])), Ok(None));
        assert_eq!(parse_args(&argv(&["mixed-forest", "--help"])), Ok(None));
    }

    #[test]
    fn missing_scenario_is_a_run_with_no_name() {
        assert_eq!(
            parse_args(&argv(&[])),
            Ok(Some(Args {
                scenario: None,
                port: None,
                ttl_seconds: 0,
                keep: false,
            }))
        );
    }

    #[test]
    fn rejects_an_unknown_flag() {
        assert!(parse_args(&argv(&["mixed-forest", "--nope"])).is_err());
    }

    #[test]
    fn rejects_a_flag_missing_its_value() {
        assert!(parse_args(&argv(&["mixed-forest", "--port"])).is_err());
    }

    #[test]
    fn rejects_a_non_numeric_port_or_ttl() {
        assert!(parse_args(&argv(&["mixed-forest", "--port", "abc"])).is_err());
        assert!(parse_args(&argv(&["mixed-forest", "--ttl", "-5"])).is_err());
    }

    #[test]
    fn rejects_a_second_positional() {
        assert!(parse_args(&argv(&["mixed-forest", "extra"])).is_err());
    }

    #[test]
    fn the_listing_carries_the_usage_line_and_every_name() {
        let listing = catalog_listing();
        assert!(listing.contains(USAGE));
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

    #[tokio::test]
    async fn binds_the_preferred_port_when_it_is_free() {
        // Reserve a port to learn a free number, release it, then confirm the
        // harness binds exactly that preferred port rather than moving off it.
        let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let free = probe.local_addr().unwrap().port();
        drop(probe);

        let listener = bind_harness_listener(None, free).await.unwrap();
        assert_eq!(listener.local_addr().unwrap().port(), free);
    }

    #[tokio::test]
    async fn falls_back_to_an_ephemeral_port_when_the_preferred_one_is_taken() {
        // Hold the preferred port, then ask the harness to prefer it. With no
        // explicit --port it binds a different OS-assigned port instead of
        // failing.
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let taken = held.local_addr().unwrap().port();

        let listener = bind_harness_listener(None, taken).await.unwrap();
        assert_ne!(listener.local_addr().unwrap().port(), taken);
    }

    #[tokio::test]
    async fn an_explicit_port_conflict_is_surfaced_as_an_error() {
        // An explicit --port is exact: a conflict must error, not silently move
        // to another port the way the defaulting path does.
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let taken = held.local_addr().unwrap().port();

        assert!(bind_harness_listener(Some(taken), 0).await.is_err());
    }
}
