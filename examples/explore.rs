//! Disposable, seeded instance of the real server for eyeballing the UI.
//!
//! `cargo run --example explore -- <scenario>` seeds a synthetic library into a
//! temp directory, serves it through the production `web::router` and `AppState`
//! unchanged, and tears the directory down on Ctrl-C. There are no assertions and
//! no browser automation: it just lets you click around a catalog of known
//! library states. This is the repeatable UI harness listed under the README's
//! "Future Work". Loose root audio surfaces the root itself (see
//! docs/adr/0005-library-root-itself-flaggable.md).

use std::process::ExitCode;

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

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&argv) {
        // Help: print the usage line and exit cleanly. The full catalog listing
        // replaces this bare line in Task 2.
        Ok(None) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        // Temporary: prove the parse wired up. Replaced by the lifecycle in Task 2.
        Ok(Some(args)) => {
            println!("{args:?}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
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
}
