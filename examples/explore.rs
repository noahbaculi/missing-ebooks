//! Disposable, seeded instance of the real server for eyeballing the UI.
//!
//! `cargo run --example explore -- <scenario>` seeds a synthetic library into a
//! temp directory, serves it through the production `web::router` and `AppState`
//! unchanged, and tears the directory down on Ctrl-C. There are no assertions and
//! no browser automation: it just lets you click around a catalog of known
//! library states. This is the repeatable UI harness listed under the README's
//! "Future Work". Loose root audio surfaces the root itself (see
//! docs/adr/0005-library-root-itself-flaggable.md).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use missing_ebooks::config::Config;
use missing_ebooks::scanner::ScanSettings;
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
            let value = iter.next().ok_or_else(|| "--port needs a value".to_string())?;
            port = Some(parse_port(value)?);
        } else if let Some(value) = arg.strip_prefix("--port=") {
            port = Some(parse_port(value)?);
        } else if arg == "--ttl" {
            let value = iter.next().ok_or_else(|| "--ttl needs a value".to_string())?;
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

/// One catalog entry: a name, a one-line description, and the builder that seeds
/// it. The builder creates the library tree or trees under the given base
/// directory and returns the library roots to configure, in render order.
#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    description: &'static str,
    build: fn(&Path) -> Vec<PathBuf>,
}

/// The shipped scenarios, in listing order.
fn catalog() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "mixed-forest",
            description: "Flagship nested tree: containers, flagged leaves, query cleaning, ancestor coverage (single root)",
            build: build_mixed_forest,
        },
        Scenario {
            name: "clean-error",
            description: "Two roots side by side: one fully covered (Clean), one uncreated (Error)",
            build: build_clean_error,
        },
        Scenario {
            name: "root-flagged",
            description: "Loose audio in the root, so the root itself is flagged (single root)",
            build: build_root_flagged,
        },
        Scenario {
            name: "pre-marked",
            description: "Pre-existing markers hide covered folders; siblings stay click targets (single root)",
            build: build_pre_marked,
        },
    ]
}

/// Resolve a scenario by exact name.
fn find_scenario(name: &str) -> Option<Scenario> {
    catalog().into_iter().find(|scenario| scenario.name == name)
}

/// The usage line followed by every scenario name and its description. Printed to
/// stdout for `--help` and to stderr for a missing or unknown scenario name.
fn catalog_listing() -> String {
    let mut out = String::new();
    out.push_str(USAGE);
    out.push_str("\n\nscenarios:\n");
    for scenario in catalog() {
        out.push_str(&format!("  {:<14}{}\n", scenario.name, scenario.description));
    }
    out
}

/// Create `dir` and every parent. Panics on failure: this is a dev tool seeding a
/// fresh temp directory, so a failure here is a bug, not a runtime condition.
fn mkdirs(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create scenario directory");
}

/// Create an empty file at `path`, creating its parents first. Mirrors the
/// `touch` helper duplicated across the unit-test modules.
fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        mkdirs(parent);
    }
    std::fs::write(path, b"").expect("write scenario file");
}

// --- Scenario builders. Each one seeds a synthetic library and returns its
// roots. They start as empty stubs and are filled in one per task. ---

fn build_mixed_forest(base: &Path) -> Vec<PathBuf> {
    let root = base.join("Library");
    // Andy Weir: a flagged leaf whose "(Unabridged)" suffix is stripped from the
    // search query, plus a sibling covered by its own epub so it drops out.
    touch(&root.join("Andy Weir/Artemis (Unabridged)/01 - Artemis.mp3"));
    touch(&root.join("Andy Weir/The Martian/01 - The Martian.m4b"));
    touch(&root.join("Andy Weir/The Martian/The Martian.epub"));
    // Cixin Liu: a series-level epub covers the whole subtree, so the author is
    // absent entirely.
    touch(&root.join("Cixin Liu/Remembrance of Earth's Past/Remembrance of Earth's Past.epub"));
    touch(&root.join(
        "Cixin Liu/Remembrance of Earth's Past/1 - The Three-Body Problem/01 - The Three-Body Problem.mp3",
    ));
    touch(&root.join(
        "Cixin Liu/Remembrance of Earth's Past/2 - The Dark Forest/01 - The Dark Forest.mp3",
    ));
    // Brandon Sanderson: two flagged leaves under a nested series container; the
    // "[2007]" segment is stripped from the second one's search query.
    touch(&root.join(
        "Brandon Sanderson/The Mistborn Saga/Mistborn 01 - The Final Empire/01 - The Final Empire.m4b",
    ));
    touch(&root.join(
        "Brandon Sanderson/The Mistborn Saga/Mistborn 02 - The Well of Ascension [2007]/01 - The Well of Ascension.m4b",
    ));
    // Robin Hobb: the U+2019 right single quotation mark in the folder name
    // survives cleaning and is percent-encoded in the search href.
    touch(&root.join(
        "Robin Hobb/Farseer Trilogy/1 - Assassin\u{2019}s Apprentice/01 - Assassin\u{2019}s Apprentice.m4b",
    ));
    vec![root]
}

fn build_clean_error(base: &Path) -> Vec<PathBuf> {
    // Root 1: every audio folder has an ebook beside it, so the root is Clean.
    let covered = base.join("Covered Library");
    touch(&covered.join("Author/Book/01 - Book.mp3"));
    touch(&covered.join("Author/Book/Book.epub"));
    // Root 2: a path we hand to library_roots but never create on disk. It cannot
    // canonicalize, so the section renders "Could not scan this root" and the
    // server logs the skip warning.
    let missing = base.join("Missing Library");
    vec![covered, missing]
}

fn build_root_flagged(_base: &Path) -> Vec<PathBuf> {
    Vec::new()
}

fn build_pre_marked(_base: &Path) -> Vec<PathBuf> {
    Vec::new()
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
    let scenario = match args.scenario.as_deref().and_then(find_scenario) {
        Some(scenario) => scenario,
        None => {
            eprint!("{}", catalog_listing());
            return ExitCode::from(2);
        }
    };

    // Initialize tracing only once we are committed to serving, so the warnings
    // the scanner emits (an unreadable root, for example) are visible.
    tracing_subscriber::fmt::init();

    let temp = match tempfile::TempDir::new() {
        Ok(temp) => temp,
        Err(err) => {
            eprintln!("could not create a temp directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let roots = (scenario.build)(temp.path());

    // The real server config, defaulted except for the seeded roots and the TTL.
    // The default search links stay so the row links render. Config.port is never
    // consulted: the example binds its own listener below.
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
    let app = web::router(state);

    // Bind 127.0.0.1, defaulting to an OS-assigned ephemeral port so we never
    // collide with a production instance on 8080. --port pins a stable URL.
    let requested = SocketAddr::from((Ipv4Addr::LOCALHOST, args.port.unwrap_or(0)));
    let listener = match TcpListener::bind(requested).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("could not bind {requested}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let addr = listener.local_addr().expect("a bound listener has a local address");
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

    use std::collections::BTreeSet;

    use missing_ebooks::scanner;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// The set of flagged folders the production scanner reports for a seeded
    /// root, as `/`-joined relative paths. This pins each scenario's intended
    /// state against the real scan logic, so a folder-name edit that quietly
    /// changes coverage fails a test instead of silently changing the UI.
    fn flagged(root: &Path) -> BTreeSet<String> {
        let settings = ScanSettings::compile(Config::default().scan_inputs()).unwrap();
        scanner::scan(root, &settings)
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn mixed_forest_flags_the_expected_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let roots = build_mixed_forest(dir.path());
        assert_eq!(roots.len(), 1);
        let want: BTreeSet<String> = [
            "Andy Weir/Artemis (Unabridged)",
            "Brandon Sanderson/The Mistborn Saga/Mistborn 01 - The Final Empire",
            "Brandon Sanderson/The Mistborn Saga/Mistborn 02 - The Well of Ascension [2007]",
            "Robin Hobb/Farseer Trilogy/1 - Assassin\u{2019}s Apprentice",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(flagged(&roots[0]), want);
    }

    #[test]
    fn clean_error_has_a_covered_root_and_an_uncreated_root() {
        let dir = tempfile::tempdir().unwrap();
        let roots = build_clean_error(dir.path());
        assert_eq!(roots.len(), 2);
        // Root 1 exists and is fully covered, so the scanner reports no gaps.
        assert!(flagged(&roots[0]).is_empty());
        assert!(roots[0].is_dir());
        // Root 2 is intentionally never created: it cannot canonicalize, which is
        // what drives the Error state in the UI.
        assert!(!roots[1].exists());
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
    fn catalog_lists_all_four_scenarios() {
        let names: Vec<&str> = catalog().iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec!["mixed-forest", "clean-error", "root-flagged", "pre-marked"]
        );
    }

    #[test]
    fn find_scenario_matches_by_name_and_rejects_unknown() {
        assert!(find_scenario("pre-marked").is_some());
        assert!(find_scenario("does-not-exist").is_none());
    }

    #[test]
    fn the_listing_carries_the_usage_line_and_every_name() {
        let listing = catalog_listing();
        assert!(listing.contains(USAGE));
        for name in ["mixed-forest", "clean-error", "root-flagged", "pre-marked"] {
            assert!(listing.contains(name), "listing is missing {name}");
        }
    }
}
