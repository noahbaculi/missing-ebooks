//! Synthetic shape sweep over the tree builders. Generates `Vec<ScannedFolder>`
//! in memory, then times the gaps render (`reduce_to_flagged` + `tree::build`)
//! and the full render (`tree::build` over the unfiltered input) across a small
//! grid of total folders, depth, fanout, and gap rate. No filesystem, no
//! network: this isolates the render cost the unified-cache rework moves onto
//! the request path.
//!
//! `cargo run --release --example tree_bench` runs the default sweep and prints
//! one row per shape combination. CLI flags pin individual axes for ad hoc
//! probes.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use missing_ebooks::scanner::{self, ScannedFolder};
use missing_ebooks::tree;

/// Round to three decimals, matching scan_bench's convention.
fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Median of the samples in milliseconds. Empty input is reported as zero, which
/// only arises in degenerate cases; every shape runs at least one iteration.
fn median(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mid = if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    };
    round3(mid)
}

/// One sweep cell: which shape was generated and how the two renders timed.
#[derive(Debug)]
struct Row {
    total: usize,
    depth: usize,
    fanout: usize,
    gap_rate: f64,
    build_gaps_ms: f64,
    build_all_ms: f64,
}

/// CLI knobs. Each is `Some` to pin a single value, `None` to sweep its default.
#[derive(Debug, PartialEq)]
struct Args {
    total: Option<usize>,
    depth: Option<usize>,
    fanout: Option<usize>,
    gap_rate: Option<f64>,
    iterations: usize,
}

const USAGE: &str = "usage: cargo run --release --example tree_bench -- \
[--total N] [--depth N] [--fanout N] [--gap-rate F] [--iterations N]";

fn help_text() -> String {
    format!(
        "{USAGE}

Synthetic shape sweep over the tree builders. No filesystem, no scenarios.

flags:
  --total N        total folders to generate (default sweep 100, 1000, 10000)
  --depth N        directory nesting depth (default sweep 2, 4, 8)
  --fanout N       children per intermediate node (default sweep 3, 10, 50)
  --gap-rate F     fraction of leaf folders that are gaps (default 0.5)
  --iterations N   measured render passes per shape (default 5)
  --help, -h       this message"
    )
}

fn next_value<'a>(
    iter: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<String, String> {
    iter.next()
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_args(argv: &[String]) -> Result<Option<Args>, String> {
    let mut total = None;
    let mut depth = None;
    let mut fanout = None;
    let mut gap_rate = None;
    let mut iterations = 5usize;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg == "--total" {
            total = Some(
                next_value(&mut iter, "--total")?
                    .parse()
                    .map_err(|e| format!("--total: {e}"))?,
            );
        } else if let Some(v) = arg.strip_prefix("--total=") {
            total = Some(v.parse().map_err(|e| format!("--total: {e}"))?);
        } else if arg == "--depth" {
            depth = Some(
                next_value(&mut iter, "--depth")?
                    .parse()
                    .map_err(|e| format!("--depth: {e}"))?,
            );
        } else if let Some(v) = arg.strip_prefix("--depth=") {
            depth = Some(v.parse().map_err(|e| format!("--depth: {e}"))?);
        } else if arg == "--fanout" {
            fanout = Some(
                next_value(&mut iter, "--fanout")?
                    .parse()
                    .map_err(|e| format!("--fanout: {e}"))?,
            );
        } else if let Some(v) = arg.strip_prefix("--fanout=") {
            fanout = Some(v.parse().map_err(|e| format!("--fanout: {e}"))?);
        } else if arg == "--gap-rate" {
            gap_rate = Some(
                next_value(&mut iter, "--gap-rate")?
                    .parse()
                    .map_err(|e| format!("--gap-rate: {e}"))?,
            );
        } else if let Some(v) = arg.strip_prefix("--gap-rate=") {
            gap_rate = Some(v.parse().map_err(|e| format!("--gap-rate: {e}"))?);
        } else if arg == "--iterations" {
            iterations = next_value(&mut iter, "--iterations")?
                .parse()
                .map_err(|e| format!("--iterations: {e}"))?;
            if iterations == 0 {
                return Err("--iterations must be at least 1".to_string());
            }
        } else if let Some(v) = arg.strip_prefix("--iterations=") {
            iterations = v.parse().map_err(|e| format!("--iterations: {e}"))?;
            if iterations == 0 {
                return Err("--iterations must be at least 1".to_string());
            }
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag {arg:?}"));
        } else {
            return Err(format!("unexpected positional argument {arg:?}"));
        }
    }
    if let Some(rate) = gap_rate
        && !(0.0..=1.0).contains(&rate)
    {
        return Err("--gap-rate must be between 0 and 1 inclusive".to_string());
    }
    Ok(Some(Args {
        total,
        depth,
        fanout,
        gap_rate,
        iterations,
    }))
}

/// Generate a synthetic `Vec<ScannedFolder>` shaped to the knobs. Only leaf folders
/// carry `directly_holds_audio = true`; coverage is applied at the leaf per
/// `gap_rate` (gap leaves are uncovered, the rest are covered). Intermediate
/// containers carry both facts off, matching what the real scanner produces above
/// a book folder. `cover_files` and `audio_files` are bounded so the renderer's
/// per-folder Vec allocations stay representative of production.
fn generate(total: usize, depth: usize, fanout: usize, gap_rate: f64) -> Vec<ScannedFolder> {
    let mut out: Vec<ScannedFolder> = Vec::with_capacity(total);
    if total == 0 || depth == 0 || fanout == 0 {
        return out;
    }
    let mut frontier: Vec<(String, usize)> = vec![(String::new(), 0)];
    let mut emitted = 0usize;
    let gap_stride = if gap_rate > 0.0 {
        (1.0 / gap_rate).round().max(1.0) as usize
    } else {
        usize::MAX
    };
    let mut leaf_index = 0usize;
    while !frontier.is_empty() && emitted < total {
        let mut next: Vec<(String, usize)> = Vec::new();
        for (parent, level) in frontier.drain(..) {
            for child in 0..fanout {
                if emitted >= total {
                    break;
                }
                let name = if level == 0 {
                    format!("Author {child:04}")
                } else {
                    format!("{parent}/Item {child:04}")
                };
                let is_leaf = level + 1 == depth;
                if is_leaf {
                    let is_gap = leaf_index.is_multiple_of(gap_stride);
                    leaf_index += 1;
                    out.push(ScannedFolder {
                        rel_path: PathBuf::from(&name),
                        directly_holds_audio: true,
                        missing_ebook: is_gap,
                        cover_files: if is_gap {
                            Vec::new()
                        } else {
                            vec!["Book.epub".to_string()]
                        },
                        audio_files: vec!["01.mp3".to_string(), "02.mp3".to_string()],
                    });
                } else {
                    out.push(ScannedFolder {
                        rel_path: PathBuf::from(&name),
                        directly_holds_audio: false,
                        missing_ebook: true,
                        cover_files: Vec::new(),
                        audio_files: Vec::new(),
                    });
                    next.push((name, level + 1));
                }
                emitted += 1;
            }
        }
        frontier = next;
    }
    out
}

/// Time one shape: build the input once, then run the two renders `iterations`
/// times each. The medians are reported.
fn run_shape(total: usize, depth: usize, fanout: usize, gap_rate: f64, iterations: usize) -> Row {
    let folders = generate(total, depth, fanout, gap_rate);
    let root_name = "Audiobooks";

    let mut gaps_samples: Vec<f64> = Vec::with_capacity(iterations);
    let mut all_samples: Vec<f64> = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let gaps_start = Instant::now();
        let flagged = scanner::reduce_to_flagged(&folders);
        let forest = tree::build(root_name, &flagged);
        gaps_samples.push(round3(gaps_start.elapsed().as_secs_f64() * 1000.0));
        std::hint::black_box(&forest);

        let all_start = Instant::now();
        let forest = tree::build(root_name, &folders);
        all_samples.push(round3(all_start.elapsed().as_secs_f64() * 1000.0));
        std::hint::black_box(&forest);
    }

    Row {
        total,
        depth,
        fanout,
        gap_rate,
        build_gaps_ms: median(&gaps_samples),
        build_all_ms: median(&all_samples),
    }
}

const TOTAL_SWEEP: &[usize] = &[100, 1_000, 10_000];
const DEPTH_SWEEP: &[usize] = &[2, 4, 8];
const FANOUT_SWEEP: &[usize] = &[3, 10, 50];
const DEFAULT_GAP_RATE: f64 = 0.5;

fn print_table(rows: &[Row]) {
    println!(
        "{:<7} {:<6} {:<7} {:<9} {:<14} {:<13}",
        "total", "depth", "fanout", "gap_rate", "build_gaps_ms", "build_all_ms"
    );
    for row in rows {
        println!(
            "{:<7} {:<6} {:<7} {:<9.2} {:<14} {:<13}",
            row.total, row.depth, row.fanout, row.gap_rate, row.build_gaps_ms, row.build_all_ms
        );
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{}", help_text());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{}", help_text());
            return ExitCode::from(2);
        }
    };

    let totals: Vec<usize> = args
        .total
        .map(|t| vec![t])
        .unwrap_or_else(|| TOTAL_SWEEP.to_vec());
    let depths: Vec<usize> = args
        .depth
        .map(|d| vec![d])
        .unwrap_or_else(|| DEPTH_SWEEP.to_vec());
    let fanouts: Vec<usize> = args
        .fanout
        .map(|f| vec![f])
        .unwrap_or_else(|| FANOUT_SWEEP.to_vec());
    let gap_rate = args.gap_rate.unwrap_or(DEFAULT_GAP_RATE);

    let mut rows = Vec::new();
    for &total in &totals {
        for &depth in &depths {
            for &fanout in &fanouts {
                rows.push(run_shape(total, depth, fanout, gap_rate, args.iterations));
            }
        }
    }
    print_table(&rows);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_respects_total_cap() {
        let folders = generate(100, 4, 10, 0.5);
        assert!(folders.len() <= 100);
    }

    #[test]
    fn generate_emits_only_leaf_audio_at_full_depth() {
        let folders = generate(50, 3, 3, 0.5);
        for f in &folders {
            let components = f.rel_path.components().count();
            if f.directly_holds_audio {
                assert_eq!(components, 3, "audio only at leaves of depth 3");
            } else {
                assert!(components < 3, "containers sit above the leaf level");
            }
        }
    }

    #[test]
    fn generate_handles_zero_knobs() {
        assert!(generate(0, 3, 3, 0.5).is_empty());
        assert!(generate(10, 0, 3, 0.5).is_empty());
        assert!(generate(10, 3, 0, 0.5).is_empty());
    }

    #[test]
    fn generate_alternates_gap_and_covered_at_half_rate() {
        let folders = generate(20, 2, 4, 0.5);
        let leaves: Vec<&ScannedFolder> =
            folders.iter().filter(|f| f.directly_holds_audio).collect();
        assert!(!leaves.is_empty());
        let mut gaps = 0;
        let mut covered = 0;
        for leaf in &leaves {
            if leaf.missing_ebook {
                gaps += 1;
                assert!(leaf.cover_files.is_empty(), "a gap has no cover files");
            } else {
                covered += 1;
                assert!(!leaf.cover_files.is_empty(), "a covered leaf has a cover");
            }
        }
        assert!(gaps > 0 && covered > 0, "both kinds appear at rate 0.5");
    }

    #[test]
    fn run_shape_returns_finite_medians() {
        let row = run_shape(50, 3, 3, 0.5, 2);
        assert!(row.build_gaps_ms.is_finite());
        assert!(row.build_all_ms.is_finite());
        assert_eq!(row.total, 50);
    }

    #[test]
    fn parse_args_accepts_defaults() {
        assert_eq!(
            parse_args(&[]),
            Ok(Some(Args {
                total: None,
                depth: None,
                fanout: None,
                gap_rate: None,
                iterations: 5,
            }))
        );
    }

    #[test]
    fn parse_args_rejects_out_of_range_gap_rate() {
        assert!(parse_args(&["--gap-rate".to_string(), "1.5".to_string()]).is_err());
        assert!(parse_args(&["--gap-rate=-0.1".to_string()]).is_err());
    }

    #[test]
    fn parse_args_pins_individual_axes() {
        let argv: Vec<String> = ["--total=200", "--depth=3", "--fanout=5", "--gap-rate=0.25"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parsed = parse_args(&argv).unwrap().unwrap();
        assert_eq!(parsed.total, Some(200));
        assert_eq!(parsed.depth, Some(3));
        assert_eq!(parsed.fanout, Some(5));
        assert_eq!(parsed.gap_rate, Some(0.25));
    }
}
