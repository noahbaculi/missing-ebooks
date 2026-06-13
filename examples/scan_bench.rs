//! Read-only benchmark: time the real scanner against the configured library
//! roots, local disk versus an SMB (CIFS) mount.
//!
//! `cargo run --release --example scan_bench -- --config config.toml --label smb --drop-caches`
//! loads the real `Config`, compiles `ScanSettings`, and times `scanner::scan`
//! (gaps-only) and `scanner::scan_all` (full walk) per root, in cold and warm
//! cache conditions, then saves a JSON report. The walks only read directory
//! entries and names; nothing here writes to the library. The single privileged
//! action is the optional `--drop-caches` page-cache flush on Linux.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;

/// Round to three decimals so the report and stdout stay readable; sub-millisecond
/// warm scans still keep enough precision to compare.
fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Median of the samples in milliseconds. Empty input is reported as zero, which
/// only arises in degenerate cases since every phase runs at least one iteration.
fn median(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        round3((v[n / 2 - 1] + v[n / 2]) / 2.0)
    }
}

fn min_of(samples: &[f64]) -> f64 {
    samples.iter().copied().reduce(f64::min).unwrap_or(0.0)
}

fn max_of(samples: &[f64]) -> f64 {
    samples.iter().copied().reduce(f64::max).unwrap_or(0.0)
}

/// Per-directory latency from the median, defined only when the walk reported a
/// positive directory count. The gaps walk has no per-directory count, so it
/// returns `None`.
fn per_dir_ms(median_ms: f64, dirs: Option<usize>) -> Option<f64> {
    match dirs {
        Some(n) if n > 0 => Some(round3(median_ms / n as f64)),
        _ => None,
    }
}

/// One cache condition's timing summary for one root and mode.
#[derive(Debug, Serialize)]
struct PhaseReport {
    /// Each measured iteration's wall-clock, in milliseconds, in run order.
    iterations_ms: Vec<f64>,
    median_ms: f64,
    min_ms: f64,
    max_ms: f64,
    /// Median divided by directories walked; `None` for the gaps walk.
    ms_per_dir: Option<f64>,
}

/// Summarize one phase's samples. `dirs` is the directory count for the median's
/// per-directory figure; pass `None` to leave it out.
fn phase_report(samples: &[f64], dirs: Option<usize>) -> PhaseReport {
    let median_ms = median(samples);
    PhaseReport {
        iterations_ms: samples.to_vec(),
        median_ms,
        min_ms: min_of(samples),
        max_ms: max_of(samples),
        ms_per_dir: per_dir_ms(median_ms, dirs),
    }
}

/// Which walk to time. `scan` (gaps-only) prunes the descent on coverage; `scan_all`
/// (full) visits and records every directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gaps,
    All,
}

impl Mode {
    /// The lowercase name used in stdout and as the report's mode key.
    fn label(self) -> &'static str {
        match self {
            Mode::Gaps => "gaps",
            Mode::All => "all",
        }
    }
}

/// Map `--mode`. `both` lists `All` first so the full walk's per-directory figure
/// prints before the gaps walk.
fn parse_modes(value: &str) -> Result<Vec<Mode>, String> {
    match value {
        "gaps" => Ok(vec![Mode::Gaps]),
        "all" => Ok(vec![Mode::All]),
        "both" => Ok(vec![Mode::All, Mode::Gaps]),
        other => Err(format!("--mode: {other:?} must be gaps, all, or both")),
    }
}

/// Parse `--iterations`; at least one measured run is required.
fn parse_iterations(value: &str) -> Result<usize, String> {
    let n: usize = value
        .parse()
        .map_err(|_| format!("--iterations: {value:?} is not a number"))?;
    if n == 0 {
        return Err("--iterations must be at least 1".to_string());
    }
    Ok(n)
}

/// A parsed command line. Defaults: five iterations, both walks, warm-only (no
/// cache drop), save the report.
#[derive(Debug, PartialEq)]
struct Args {
    config: Option<PathBuf>,
    roots: Vec<PathBuf>,
    iterations: usize,
    modes: Vec<Mode>,
    drop_caches: bool,
    label: Option<String>,
    out: Option<PathBuf>,
    no_save: bool,
}

/// Pull the value that follows a space-form flag, erroring if the vector ends.
fn next_value<'a>(
    iter: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<String, String> {
    iter.next()
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Parse the argument vector (already stripped of the program name). `Ok(None)`
/// means help was requested; `Ok(Some(args))` is a run request; `Err(message)` is
/// a usage error the caller prints beside the help text. Hand-rolled to match
/// `explore.rs`; no clap.
fn parse_args(argv: &[String]) -> Result<Option<Args>, String> {
    let mut config = None;
    let mut roots = Vec::new();
    let mut iterations = 5usize;
    let mut modes = vec![Mode::All, Mode::Gaps];
    let mut drop_caches = false;
    let mut label = None;
    let mut out = None;
    let mut no_save = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg == "--drop-caches" {
            drop_caches = true;
        } else if arg == "--no-save" {
            no_save = true;
        } else if arg == "--config" {
            config = Some(PathBuf::from(next_value(&mut iter, "--config")?));
        } else if let Some(v) = arg.strip_prefix("--config=") {
            config = Some(PathBuf::from(v));
        } else if arg == "--root" {
            roots.push(PathBuf::from(next_value(&mut iter, "--root")?));
        } else if let Some(v) = arg.strip_prefix("--root=") {
            roots.push(PathBuf::from(v));
        } else if arg == "--iterations" {
            iterations = parse_iterations(&next_value(&mut iter, "--iterations")?)?;
        } else if let Some(v) = arg.strip_prefix("--iterations=") {
            iterations = parse_iterations(v)?;
        } else if arg == "--mode" {
            modes = parse_modes(&next_value(&mut iter, "--mode")?)?;
        } else if let Some(v) = arg.strip_prefix("--mode=") {
            modes = parse_modes(v)?;
        } else if arg == "--label" {
            label = Some(next_value(&mut iter, "--label")?);
        } else if let Some(v) = arg.strip_prefix("--label=") {
            label = Some(v.to_string());
        } else if arg == "--out" {
            out = Some(PathBuf::from(next_value(&mut iter, "--out")?));
        } else if let Some(v) = arg.strip_prefix("--out=") {
            out = Some(PathBuf::from(v));
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag {arg:?}"));
        } else {
            return Err(format!("unexpected positional argument {arg:?}"));
        }
    }
    Ok(Some(Args {
        config,
        roots,
        iterations,
        modes,
        drop_caches,
        label,
        out,
        no_save,
    }))
}

fn main() -> ExitCode {
    // Wired in Task 11. The skeleton compiles so earlier tasks can add and test
    // pure helpers against a real target.
    eprintln!("scan_bench is not wired up yet");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round3_keeps_three_decimals() {
        assert_eq!(round3(1.23456), 1.235);
        assert_eq!(round3(9140.0), 9140.0);
    }

    #[test]
    fn median_handles_odd_and_even_lengths() {
        assert_eq!(median(&[30.0, 10.0, 20.0]), 20.0);
        assert_eq!(median(&[10.0, 20.0, 30.0, 40.0]), 25.0);
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn min_and_max_pick_the_extremes() {
        assert_eq!(min_of(&[30.0, 10.0, 20.0]), 10.0);
        assert_eq!(max_of(&[30.0, 10.0, 20.0]), 30.0);
        assert_eq!(min_of(&[]), 0.0);
        assert_eq!(max_of(&[]), 0.0);
    }

    #[test]
    fn per_dir_ms_divides_only_with_a_positive_count() {
        assert_eq!(per_dir_ms(100.0, Some(50)), Some(2.0));
        assert_eq!(per_dir_ms(100.0, Some(0)), None);
        assert_eq!(per_dir_ms(100.0, None), None);
    }

    #[test]
    fn phase_report_aggregates_samples_with_per_dir() {
        let p = phase_report(&[10.0, 20.0, 30.0], Some(10));
        assert_eq!(p.iterations_ms, vec![10.0, 20.0, 30.0]);
        assert_eq!(p.median_ms, 20.0);
        assert_eq!(p.min_ms, 10.0);
        assert_eq!(p.max_ms, 30.0);
        assert_eq!(p.ms_per_dir, Some(2.0));
    }

    #[test]
    fn phase_report_leaves_per_dir_none_without_a_count() {
        let p = phase_report(&[10.0, 20.0], None);
        assert_eq!(p.median_ms, 15.0);
        assert_eq!(p.ms_per_dir, None);
    }

    #[test]
    fn parse_modes_maps_each_keyword() {
        assert_eq!(parse_modes("gaps"), Ok(vec![Mode::Gaps]));
        assert_eq!(parse_modes("all"), Ok(vec![Mode::All]));
        assert_eq!(parse_modes("both"), Ok(vec![Mode::All, Mode::Gaps]));
        assert!(parse_modes("nope").is_err());
    }

    #[test]
    fn parse_iterations_rejects_zero_and_non_numbers() {
        assert_eq!(parse_iterations("5"), Ok(5));
        assert!(parse_iterations("0").is_err());
        assert!(parse_iterations("abc").is_err());
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_defaults_with_no_flags() {
        assert_eq!(
            parse_args(&argv(&[])),
            Ok(Some(Args {
                config: None,
                roots: vec![],
                iterations: 5,
                modes: vec![Mode::All, Mode::Gaps],
                drop_caches: false,
                label: None,
                out: None,
                no_save: false,
            }))
        );
    }

    #[test]
    fn parses_every_flag_in_space_form() {
        let parsed = parse_args(&argv(&[
            "--config", "config.toml",
            "--root", "/mnt/a",
            "--root", "/mnt/b",
            "--iterations", "3",
            "--mode", "all",
            "--drop-caches",
            "--label", "smb",
            "--out", "out.json",
            "--no-save",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.config, Some(std::path::PathBuf::from("config.toml")));
        assert_eq!(parsed.roots, vec![
            std::path::PathBuf::from("/mnt/a"),
            std::path::PathBuf::from("/mnt/b"),
        ]);
        assert_eq!(parsed.iterations, 3);
        assert_eq!(parsed.modes, vec![Mode::All]);
        assert!(parsed.drop_caches);
        assert_eq!(parsed.label.as_deref(), Some("smb"));
        assert_eq!(parsed.out, Some(std::path::PathBuf::from("out.json")));
        assert!(parsed.no_save);
    }

    #[test]
    fn parses_flags_in_equals_form() {
        let parsed = parse_args(&argv(&["--config=config.toml", "--mode=gaps", "--iterations=2"]))
            .unwrap()
            .unwrap();
        assert_eq!(parsed.config, Some(std::path::PathBuf::from("config.toml")));
        assert_eq!(parsed.modes, vec![Mode::Gaps]);
        assert_eq!(parsed.iterations, 2);
    }

    #[test]
    fn help_short_circuits_to_none() {
        assert_eq!(parse_args(&argv(&["--help"])), Ok(None));
        assert_eq!(parse_args(&argv(&["-h"])), Ok(None));
    }

    #[test]
    fn rejects_unknown_flag_missing_value_and_positional() {
        assert!(parse_args(&argv(&["--nope"])).is_err());
        assert!(parse_args(&argv(&["--config"])).is_err());
        assert!(parse_args(&argv(&["stray"])).is_err());
    }
}
