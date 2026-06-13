//! Read-only benchmark: time the real scanner against the configured library
//! roots, local disk versus an SMB (CIFS) mount.
//!
//! `cargo run --release --example scan_bench -- --config config.toml --label smb --drop-caches`
//! loads the real `Config`, compiles `ScanSettings`, and times `scanner::scan`
//! (gaps-only) and `scanner::scan_all` (full walk) per root, in cold and warm
//! cache conditions, then saves a JSON report. The walks only read directory
//! entries and names; nothing here writes to the library. The single privileged
//! action is the optional `--drop-caches` page-cache flush on Linux.

use std::process::ExitCode;

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
}
