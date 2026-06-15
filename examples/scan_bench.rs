//! Read-only benchmark: time the real scanner against the configured library
//! roots, local disk versus an SMB (CIFS) mount.
//!
//! `cargo run --release --example scan_bench -- --config config.toml --label smb --drop-caches`
//! loads the real `Config`, compiles `ScanSettings`, and times `scanner::scan_with_stats`
//! (gaps-only) and `scanner::scan_all_with_stats` (full walk) per root, in cold and warm
//! cache conditions, then saves a JSON report. The walks only read directory
//! entries and names; nothing here writes to the library. The single privileged
//! action is the optional `--drop-caches` page-cache flush on Linux.
//!
//! On a prod box: build and run with `--release` so the timings reflect the
//! shipped binary, not a debug build. `--drop-caches` flushes the page cache for
//! the cold runs and needs root on Linux, so run that invocation under `sudo`;
//! omit the flag for warm-only numbers. With no Rust toolchain on the box, build
//! `target/release/examples/scan_bench` on a machine of matching OS and arch and
//! copy the binary over. Point it at the real mounts with `--config config.toml`,
//! repeated `--root PATH`, or `MISSING_EBOOKS_LIBRARY_ROOTS`.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use missing_ebooks::config::Config;
use missing_ebooks::scanner::{self, ScanSettings, WalkStats};
use serde::Serialize;

/// Round to three decimals so the report and stdout stay readable. Sub-millisecond
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
    let mid = if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    };
    round3(mid)
}

fn min_of(samples: &[f64]) -> f64 {
    samples.iter().copied().reduce(f64::min).unwrap_or(0.0)
}

fn max_of(samples: &[f64]) -> f64 {
    samples.iter().copied().reduce(f64::max).unwrap_or(0.0)
}

/// Per-directory latency from the median, or `None` when no directory was walked
/// (an empty or unreadable root).
fn per_dir_ms(median_ms: f64, dirs: usize) -> Option<f64> {
    (dirs > 0).then(|| round3(median_ms / dirs as f64))
}

/// One cache condition's timing summary for one root and mode.
#[derive(Debug, Serialize)]
struct PhaseReport {
    /// Each measured iteration's wall-clock, in milliseconds, in run order.
    iterations_ms: Vec<f64>,
    median_ms: f64,
    min_ms: f64,
    max_ms: f64,
    /// Median divided by directories visited. `None` only when no directory was walked.
    ms_per_dir: Option<f64>,
}

/// Summarize one phase's samples. `dirs` is the directory count for the median's
/// per-directory figure.
fn phase_report(samples: &[f64], dirs: usize) -> PhaseReport {
    let median_ms = median(samples);
    PhaseReport {
        iterations_ms: samples.to_vec(),
        median_ms,
        min_ms: min_of(samples),
        max_ms: max_of(samples),
        ms_per_dir: per_dir_ms(median_ms, dirs),
    }
}

/// Which walk to time. `Gaps` reduces the full walk to flagged folders, `Full`
/// records every directory, and `Incremental` reuses unchanged directories via the
/// mtime index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gaps,
    Full,
    Incremental,
}

impl Mode {
    /// The lowercase name used in stdout and as the report's mode key.
    fn label(self) -> &'static str {
        match self {
            Mode::Gaps => "gaps",
            Mode::Full => "full",
            Mode::Incremental => "incremental",
        }
    }
}

/// Map `--mode`: one keyword or a comma-separated list, e.g. `full,incremental`.
/// `every` expands to all modes, the comprehensive default. Duplicates collapse,
/// first occurrence wins, so the listed order is the report order. `all` is not a
/// keyword: older reports key the full walk as `all`, so it stays a report value
/// only, never a live selector.
fn parse_modes(value: &str) -> Result<Vec<Mode>, String> {
    let mut modes = Vec::new();
    for part in value.split(',') {
        let expanded = match part.trim() {
            "gaps" => vec![Mode::Gaps],
            "full" => vec![Mode::Full],
            "incremental" => vec![Mode::Incremental],
            "every" => vec![Mode::Full, Mode::Gaps, Mode::Incremental],
            other => {
                return Err(format!(
                    "--mode: {other:?} must be full, gaps, incremental, or every"
                ));
            }
        };
        for mode in expanded {
            if !modes.contains(&mode) {
                modes.push(mode);
            }
        }
    }
    if modes.is_empty() {
        return Err("--mode: at least one mode is required".to_string());
    }
    Ok(modes)
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

/// Parse `--concurrency`: a comma-separated list of positive thread counts, e.g.
/// `1,4,8,16`. Each value sizes the scan thread pool for one sweep entry.
fn parse_concurrency(value: &str) -> Result<Vec<usize>, String> {
    let mut levels = Vec::new();
    for part in value.split(',') {
        let n: usize = part
            .trim()
            .parse()
            .map_err(|_| format!("--concurrency: {part:?} is not a number"))?;
        if n == 0 {
            return Err("--concurrency values must be at least 1".to_string());
        }
        levels.push(n);
    }
    Ok(levels)
}

/// A parsed command line. Defaults: five iterations, all three walks, warm-only
/// (no cache drop), save the report.
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
    concurrency: Vec<usize>,
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
/// is a help request, `Ok(Some(args))` a run request, `Err(message)` a usage error
/// the caller prints beside the help text. Hand-rolled to match `explore.rs`, not
/// clap.
fn parse_args(argv: &[String]) -> Result<Option<Args>, String> {
    let mut config = None;
    let mut roots = Vec::new();
    let mut iterations = 5usize;
    let mut modes = vec![Mode::Full, Mode::Gaps, Mode::Incremental];
    let mut drop_caches = false;
    let mut label = None;
    let mut out = None;
    let mut no_save = false;
    let mut concurrency = vec![16usize];
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
        } else if arg == "--concurrency" {
            concurrency = parse_concurrency(&next_value(&mut iter, "--concurrency")?)?;
        } else if let Some(v) = arg.strip_prefix("--concurrency=") {
            concurrency = parse_concurrency(v)?;
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
        concurrency,
    }))
}

/// Return the `(fstype, options)` of the mount whose mount point is the longest
/// prefix of `path`, given the text of `/proc/self/mounts`. Columns are device,
/// mount point, fstype, options; a shorter line is skipped. Names the filesystem
/// under a root well enough, but does not decode the octal escapes `/proc` uses for
/// spaces in mount points.
fn mount_for_path(mounts: &str, path: &Path) -> Option<(String, String)> {
    let mut best: Option<(usize, String, String)> = None;
    for line in mounts.lines() {
        let mut cols = line.split_whitespace();
        let (_device, Some(mountpoint), Some(fstype), Some(options)) =
            (cols.next(), cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        if path.starts_with(Path::new(mountpoint)) {
            let len = mountpoint.len();
            if best.as_ref().is_none_or(|(best_len, _, _)| len > *best_len) {
                best = Some((len, fstype.to_string(), options.to_string()));
            }
        }
    }
    best.map(|(_, fstype, options)| (fstype, options))
}

/// What one walk found: the counts the report records. `stats` holds the directory
/// and entry totals from the walk itself. `gaps` and `audio_files` are derived from
/// the result after the clock stops.
struct WalkCounts {
    stats: WalkStats,
    gaps: usize,
    audio_files: usize,
}

/// The report schema version, bumped when the JSON shape changes so a directory of
/// mixed-vintage reports stays parseable. Schema 3 adds the `incremental` mode: a
/// single-level entry carrying `dirs_reused`, absent on the `full` and `gaps` modes.
const SCHEMA_VERSION: u32 = 3;

/// The whole run: environment context plus one entry per root.
#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    tool: &'static str,
    label: String,
    host: String,
    kernel: String,
    unix_time: u64,
    build_profile: &'static str,
    iterations: usize,
    drop_caches: bool,
    roots: Vec<RootReport>,
}

/// One library root: where it is, what filesystem it sits on, and its per-mode
/// timings keyed by mode label (`full`, `gaps`, `incremental`) for a stable order.
#[derive(Debug, Serialize)]
struct RootReport {
    path: String,
    fstype: String,
    mount_options: String,
    modes: BTreeMap<String, ModeReport>,
}

/// One mode's timings, one entry per swept concurrency level. A single-value
/// `--concurrency` yields a one-element vector.
#[derive(Debug, Serialize)]
struct ModeReport {
    levels: Vec<LevelReport>,
}

/// One concurrency level's counts and timings for a mode. `cold` is `None` when
/// `--drop-caches` was off. `dirs_reused` is `Some` only for the incremental mode,
/// where it counts the directories served from the index without a listing. The
/// `full` and `gaps` walks always list, so they omit it.
#[derive(Debug, Serialize)]
struct LevelReport {
    concurrency: usize,
    dirs_visited: usize,
    entries_seen: usize,
    gaps: usize,
    audio_files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    dirs_reused: Option<usize>,
    cold: Option<PhaseReport>,
    warm: PhaseReport,
}

/// The auto-generated report filename, sortable by the trailing unix seconds.
fn default_report_path(label: &str, host: &str, unix_time: u64) -> PathBuf {
    PathBuf::from(format!("scan-bench-{label}-{host}-{unix_time}.json"))
}

/// Write the report as pretty JSON. Returns the error message on failure so the
/// caller can surface it without aborting the timing it already printed.
fn write_report(report: &Report, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| format!("could not encode the report: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Read a small text file and trim it, or `None` if it is unreadable. Used for
/// the single-line `/proc` values below, where the harness degrades to a
/// placeholder rather than failing when `/proc` is absent.
fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn hostname() -> String {
    read_trimmed(Path::new("/proc/sys/kernel/hostname")).unwrap_or_else(|| "unknown".to_string())
}

fn kernel_release() -> String {
    read_trimmed(Path::new("/proc/sys/kernel/osrelease")).unwrap_or_else(|| "unknown".to_string())
}

/// `debug` or `release`, so the report flags an un-optimized local baseline.
fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Seconds since the unix epoch, for the report and its filename.
fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Flush the Linux page cache, dentries, and inodes so the next walk is cold.
/// Only this step escalates: build and run as the normal user and enter the sudo
/// password once (or pre-run `sudo -v`). The CIFS client cache lives here, so this
/// is a genuine client-side cold walk. The SMB server may still hold the tree in
/// its own RAM, so the cold number is the client's view.
fn drop_caches() -> Result<(), String> {
    let status = std::process::Command::new("sudo")
        .args(["sh", "-c", "sync && echo 3 > /proc/sys/vm/drop_caches"])
        .status()
        .map_err(|e| format!("could not run sudo to drop caches: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("drop-caches command exited with {status}"))
    }
}

const USAGE: &str = "usage: cargo run --release --example scan_bench -- \
[--config PATH] [--root PATH]... [--iterations N] [--mode LIST] \
[--concurrency LIST] [--drop-caches] [--label NAME] [--out PATH] [--no-save]";

/// The usage line plus the flag reference and the read-only and strace notes.
fn help_text() -> String {
    format!(
        "{USAGE}

Times the real scanner (read-only) against each library root and saves a JSON
report. Roots come from --root, --config, or MISSING_EBOOKS_LIBRARY_ROOTS.

flags:
  --config PATH     load the real config.toml (extensions, exclusions, roots)
  --root PATH       benchmark this exact path; repeatable; replaces config roots
  --iterations N    measured runs per phase (default 5)
  --mode LIST       comma-separated: full, gaps, incremental, or every (default every);
                    every runs all modes, so a bare run saves a comprehensive report
  --concurrency LIST   thread counts to sweep, comma-separated, e.g. 1,4,8,16 (default 16)
  --drop-caches     Linux: sudo-flush the page cache before each cold run
  --label NAME      tag stdout and the report (e.g. local, smb)
  --out PATH        report path (default scan-bench-<label>-<host>-<time>.json)
  --no-save         do not write the report file
  --help, -h        this message

The full walk reports ms/dir, the headline figure; the gaps walk reports total
time only. For a syscall-level cross-check, run once under
`strace -f -e trace=getdents64,newfstatat -c`."
    )
}

/// Measure incremental reuse and fold it into the JSON report as a single-level
/// `incremental` mode. Build the index with one full listing walk (discarded), then
/// time reuse walks against it. `--drop-caches` adds a cold phase that drops the
/// client cache before each walk, so every directory stat is a real round trip, the
/// honest figure over a network mount. The warm phase shows the cache-hot case.
/// Concurrency is inert over SMB for the reuse walk, so this runs at one `threads`
/// rather than sweeping it, recording that as the level's concurrency. The reused-vs-
/// listed split and the per-phase timings also print live as the run proceeds.
fn run_incremental(
    root: &Path,
    settings: &ScanSettings,
    iterations: usize,
    threads: usize,
    drop_caches_enabled: bool,
) -> Result<ModeReport, String> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| format!("could not build a {threads}-thread pool: {e}"))?;

    let mut index = scanner::DirIndex::new();
    // Build the index: this first walk lists everything, so its timing is discarded.
    let (_ms, mut last) = pool.install(|| time_reuse_walk(root, settings, &mut index));

    let cold = if drop_caches_enabled {
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            drop_caches()?;
            let (ms, counts) = pool.install(|| time_reuse_walk(root, settings, &mut index));
            samples.push(ms);
            last = counts;
        }
        Some(phase_report(&samples, last.stats.dirs_visited))
    } else {
        None
    };

    // Warm phase: one discarded warmup, then measured reuse walks with no drop.
    let _ = pool.install(|| time_reuse_walk(root, settings, &mut index));
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let (ms, counts) = pool.install(|| time_reuse_walk(root, settings, &mut index));
        samples.push(ms);
        last = counts;
    }
    let warm = phase_report(&samples, last.stats.dirs_visited);

    println!(
        "  mode=incremental  concurrency={threads}  dirs_visited={}  dirs_reused={}  \
         entries_seen={}  (index {} dirs)",
        last.stats.dirs_visited,
        last.stats.dirs_reused,
        last.stats.entries_seen,
        index.len()
    );
    if let Some(cold) = &cold {
        println!("{}", fmt_phase("cold", cold));
    }
    println!("{}", fmt_phase("warm", &warm));

    Ok(ModeReport {
        levels: vec![LevelReport {
            concurrency: threads,
            dirs_visited: last.stats.dirs_visited,
            entries_seen: last.stats.entries_seen,
            gaps: last.gaps,
            audio_files: last.audio_files,
            dirs_reused: Some(last.stats.dirs_reused),
            cold,
            warm,
        }],
    })
}

/// Time one reuse walk against the prebuilt `index`, returning its wall-clock in
/// milliseconds and the walk's counts. The gap and audio tallies are derived after
/// the clock stops, like `time_walk`, so the in-memory reduce never inflates the
/// measured wall-clock. On an unchanged tree every directory is reused and no entry
/// is listed.
fn time_reuse_walk(
    root: &Path,
    settings: &ScanSettings,
    index: &mut scanner::DirIndex,
) -> (f64, WalkCounts) {
    let start = Instant::now();
    let (folders, stats) = scanner::scan_all_incremental_with_stats(root, settings, index);
    let ms = round3(start.elapsed().as_secs_f64() * 1000.0);
    let gaps = folders
        .iter()
        .filter(|f| f.directly_holds_audio && f.missing_ebook)
        .count();
    let audio_files = folders.iter().map(|f| f.audio_files.len()).sum();
    (
        ms,
        WalkCounts {
            stats,
            gaps,
            audio_files,
        },
    )
}

/// Time one read-only walk and tally what it found. Only the walk sits inside the
/// `Instant`. The gap and audio counts are derived after the clock stops, so the
/// tally never inflates the measured wall-clock.
fn time_walk(mode: Mode, root: &Path, settings: &ScanSettings) -> (f64, WalkCounts) {
    match mode {
        Mode::Full => {
            let start = Instant::now();
            let (folders, stats) = scanner::scan_all_with_stats(root, settings);
            let ms = round3(start.elapsed().as_secs_f64() * 1000.0);
            let gaps = folders
                .iter()
                .filter(|f| f.directly_holds_audio && f.missing_ebook)
                .count();
            let audio_files = folders.iter().map(|f| f.audio_files.len()).sum();
            (
                ms,
                WalkCounts {
                    stats,
                    gaps,
                    audio_files,
                },
            )
        }
        Mode::Gaps => {
            let start = Instant::now();
            let (flagged, stats) = scanner::scan_with_stats(root, settings);
            let ms = round3(start.elapsed().as_secs_f64() * 1000.0);
            let audio_files = flagged.iter().map(|f| f.audio_files.len()).sum();
            (
                ms,
                WalkCounts {
                    stats,
                    gaps: flagged.len(),
                    audio_files,
                },
            )
        }
        // Incremental is routed to run_incremental before any phase calls this.
        Mode::Incremental => unreachable!("incremental mode does not use time_walk"),
    }
}

/// Cold phase: drop the cache before each of `iterations` runs, so every sample
/// is cold. Returns the summary and the counts observed.
fn cold_phase(
    mode: Mode,
    root: &Path,
    settings: &ScanSettings,
    iterations: usize,
) -> Result<(PhaseReport, WalkCounts), String> {
    let mut samples = Vec::with_capacity(iterations);
    let mut counts = None;
    for _ in 0..iterations {
        drop_caches()?;
        let (ms, c) = time_walk(mode, root, settings);
        samples.push(ms);
        counts = Some(c);
    }
    let counts = counts.expect("iterations >= 1 guarantees a sample");
    Ok((phase_report(&samples, counts.stats.dirs_visited), counts))
}

/// Warm phase: one discarded warmup run, then `iterations` consecutive measured
/// runs with no cache drop.
fn warm_phase(
    mode: Mode,
    root: &Path,
    settings: &ScanSettings,
    iterations: usize,
) -> (PhaseReport, WalkCounts) {
    let (_, counts) = time_walk(mode, root, settings);
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let (ms, _) = time_walk(mode, root, settings);
        samples.push(ms);
    }
    (phase_report(&samples, counts.stats.dirs_visited), counts)
}

/// One phase line for stdout, with the per-directory figure when present.
fn fmt_phase(name: &str, p: &PhaseReport) -> String {
    let runs: Vec<String> = p.iterations_ms.iter().map(|ms| format!("{ms}")).collect();
    let per_dir = match p.ms_per_dir {
        Some(d) => format!(" ({d} ms/dir)"),
        None => String::new(),
    };
    format!(
        "    {name}:  runs [{}] ms  ->  median {} ms{}  min {}  max {}",
        runs.join(", "),
        p.median_ms,
        per_dir,
        p.min_ms,
        p.max_ms
    )
}

/// Resolve the config: a file when `--config` is set, else env-only when no
/// `--root` was given, else defaults whose roots `--root` supplies. `--root`
/// always replaces the roots so the run benchmarks exactly the named paths.
fn resolve_config(args: &Args) -> Result<Config, String> {
    let mut config = match args.config.as_deref() {
        Some(path) => Config::load(Some(path)).map_err(|e| e.to_string())?,
        None if args.roots.is_empty() => Config::load(None).map_err(|e| e.to_string())?,
        None => Config::default(),
    };
    if !args.roots.is_empty() {
        config.library_roots = args.roots.clone();
    }
    Ok(config)
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

    // Scanner warnings (an unreadable root, a covering root) should be visible.
    tracing_subscriber::fmt::init();

    let config = match resolve_config(&args) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("error: {message}\n\n{}", help_text());
            return ExitCode::from(2);
        }
    };
    let settings = match ScanSettings::compile(config.scan_inputs()) {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("invalid scan settings: {err}");
            return ExitCode::FAILURE;
        }
    };

    let host = hostname();
    let kernel = kernel_release();
    let profile = build_profile();
    let unix = unix_time();
    let label = args
        .label
        .clone()
        .unwrap_or_else(|| "unlabeled".to_string());
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();

    println!(
        "scan_bench [{label}] host={host} kernel={kernel} profile={profile} \
         iterations={} drop_caches={}",
        args.iterations, args.drop_caches
    );
    if profile == "debug" {
        println!("  note: build with --release for an honest local baseline");
    }
    if args.drop_caches {
        println!("  note: cold means client-side cold; the SMB server may still cache the tree");
    }

    let mut roots = Vec::new();
    for root in &config.library_roots {
        // A canonicalize failure means a missing or unreadable root. Warn loudly:
        // the raw path won't match any mount (so fstype reads "unknown") and the
        // scanner skips it, which would otherwise pass for a successful zero-gap run.
        let canonical = match std::fs::canonicalize(root) {
            Ok(path) => path,
            Err(err) => {
                eprintln!(
                    "warning: could not resolve {}: {err}; \
                     timing the raw path, fstype and counts may be unreliable",
                    root.display()
                );
                root.clone()
            }
        };
        let (fstype, options) = mount_for_path(&mounts, &canonical)
            .unwrap_or_else(|| ("unknown".to_string(), String::new()));
        if options.is_empty() {
            println!("\n[{label}] {}  (fstype: {fstype})", canonical.display());
        } else {
            println!(
                "\n[{label}] {}  (fstype: {fstype}, options: {options})",
                canonical.display()
            );
        }

        let mut modes = BTreeMap::new();
        for &mode in &args.modes {
            // Incremental is a focused reuse measurement, outside the concurrency
            // sweep: it runs once at the top swept thread count (16 by default),
            // since concurrency is inert over SMB for the reuse walk.
            if mode == Mode::Incremental {
                let threads = args.concurrency.iter().copied().max().unwrap_or(16);
                match run_incremental(
                    &canonical,
                    &settings,
                    args.iterations,
                    threads,
                    args.drop_caches,
                ) {
                    Ok(report) => {
                        modes.insert(mode.label().to_string(), report);
                    }
                    Err(message) => {
                        eprintln!("error during incremental run: {message}");
                        return ExitCode::FAILURE;
                    }
                }
                continue;
            }
            let mut levels = Vec::new();
            for &threads in &args.concurrency {
                let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
                    Ok(pool) => pool,
                    Err(err) => {
                        eprintln!("error: could not build a {threads}-thread pool: {err}");
                        return ExitCode::FAILURE;
                    }
                };
                // Run the phases inside the pool so the scanner's parallel walk
                // uses exactly these `threads` workers for this sweep entry.
                let cold = if args.drop_caches {
                    match pool.install(|| cold_phase(mode, &canonical, &settings, args.iterations))
                    {
                        Ok((phase, _)) => Some(phase),
                        Err(message) => {
                            eprintln!("error during cold phase: {message}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else {
                    None
                };
                let (warm, counts) =
                    pool.install(|| warm_phase(mode, &canonical, &settings, args.iterations));

                println!(
                    "  mode={} concurrency={}  dirs_visited={}  entries_seen={}  gaps={}  audio_files={}",
                    mode.label(),
                    threads,
                    counts.stats.dirs_visited,
                    counts.stats.entries_seen,
                    counts.gaps,
                    counts.audio_files
                );
                if let Some(cold) = &cold {
                    println!("{}", fmt_phase("cold", cold));
                }
                println!("{}", fmt_phase("warm", &warm));

                levels.push(LevelReport {
                    concurrency: threads,
                    dirs_visited: counts.stats.dirs_visited,
                    entries_seen: counts.stats.entries_seen,
                    gaps: counts.gaps,
                    audio_files: counts.audio_files,
                    dirs_reused: None,
                    cold,
                    warm,
                });
            }
            modes.insert(mode.label().to_string(), ModeReport { levels });
        }

        roots.push(RootReport {
            path: canonical.display().to_string(),
            fstype,
            mount_options: options,
            modes,
        });
    }

    let report = Report {
        schema_version: SCHEMA_VERSION,
        tool: "scan_bench",
        label: label.clone(),
        host: host.clone(),
        kernel,
        unix_time: unix,
        build_profile: profile,
        iterations: args.iterations,
        drop_caches: args.drop_caches,
        roots,
    };

    if args.no_save {
        println!("\n--no-save: report not written");
    } else {
        let path = args
            .out
            .clone()
            .unwrap_or_else(|| default_report_path(&label, &host, unix));
        match write_report(&report, &path) {
            Ok(()) => println!("\nsaved report to {}", path.display()),
            Err(message) => {
                eprintln!("\n{message}");
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
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
        assert_eq!(per_dir_ms(100.0, 50), Some(2.0));
        assert_eq!(per_dir_ms(100.0, 0), None);
    }

    #[test]
    fn phase_report_aggregates_samples_with_per_dir() {
        let p = phase_report(&[10.0, 20.0, 30.0], 10);
        assert_eq!(p.iterations_ms, vec![10.0, 20.0, 30.0]);
        assert_eq!(p.median_ms, 20.0);
        assert_eq!(p.min_ms, 10.0);
        assert_eq!(p.max_ms, 30.0);
        assert_eq!(p.ms_per_dir, Some(2.0));
    }

    #[test]
    fn phase_report_leaves_per_dir_none_with_zero_dirs() {
        let p = phase_report(&[10.0, 20.0], 0);
        assert_eq!(p.median_ms, 15.0);
        assert_eq!(p.ms_per_dir, None);
    }

    #[test]
    fn parse_modes_maps_each_keyword() {
        assert_eq!(parse_modes("gaps"), Ok(vec![Mode::Gaps]));
        assert_eq!(parse_modes("full"), Ok(vec![Mode::Full]));
        assert_eq!(parse_modes("incremental"), Ok(vec![Mode::Incremental]));
        // `every` expands to all modes, in report order.
        assert_eq!(
            parse_modes("every"),
            Ok(vec![Mode::Full, Mode::Gaps, Mode::Incremental])
        );
        assert!(parse_modes("nope").is_err());
        // `both` was dropped when a third mode arrived; the comma list replaces it.
        assert!(parse_modes("both").is_err());
        // `all` is retired as a selector; it survives only as an old report's key.
        assert!(parse_modes("all").is_err());
    }

    #[test]
    fn parse_modes_reads_a_comma_list_in_order_without_duplicates() {
        assert_eq!(
            parse_modes("full,incremental"),
            Ok(vec![Mode::Full, Mode::Incremental])
        );
        // Whitespace is trimmed, and `full` overlapping `every`'s expansion collapses.
        assert_eq!(
            parse_modes("full, every "),
            Ok(vec![Mode::Full, Mode::Gaps, Mode::Incremental])
        );
        // One bad token in the list fails the whole parse.
        assert!(parse_modes("full,nope").is_err());
    }

    #[test]
    fn parse_iterations_rejects_zero_and_non_numbers() {
        assert_eq!(parse_iterations("5"), Ok(5));
        assert!(parse_iterations("0").is_err());
        assert!(parse_iterations("abc").is_err());
    }

    #[test]
    fn parse_concurrency_reads_a_list_and_rejects_bad_values() {
        assert_eq!(parse_concurrency("1,4,8,16"), Ok(vec![1, 4, 8, 16]));
        assert_eq!(parse_concurrency("8"), Ok(vec![8]));
        assert!(parse_concurrency("0").is_err());
        assert!(parse_concurrency("1,x").is_err());
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
                modes: vec![Mode::Full, Mode::Gaps, Mode::Incremental],
                drop_caches: false,
                label: None,
                out: None,
                no_save: false,
                concurrency: vec![16],
            }))
        );
    }

    #[test]
    fn parses_every_flag_in_space_form() {
        let parsed = parse_args(&argv(&[
            "--config",
            "config.toml",
            "--root",
            "/mnt/a",
            "--root",
            "/mnt/b",
            "--iterations",
            "3",
            "--mode",
            "full",
            "--drop-caches",
            "--label",
            "smb",
            "--concurrency",
            "1,4",
            "--out",
            "out.json",
            "--no-save",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.config, Some(std::path::PathBuf::from("config.toml")));
        assert_eq!(
            parsed.roots,
            vec![
                std::path::PathBuf::from("/mnt/a"),
                std::path::PathBuf::from("/mnt/b"),
            ]
        );
        assert_eq!(parsed.iterations, 3);
        assert_eq!(parsed.modes, vec![Mode::Full]);
        assert!(parsed.drop_caches);
        assert_eq!(parsed.label.as_deref(), Some("smb"));
        assert_eq!(parsed.out, Some(std::path::PathBuf::from("out.json")));
        assert!(parsed.no_save);
        assert_eq!(parsed.concurrency, vec![1, 4]);
    }

    #[test]
    fn parses_flags_in_equals_form() {
        let parsed = parse_args(&argv(&[
            "--config=config.toml",
            "--mode=gaps",
            "--iterations=2",
            "--concurrency=2,8",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.config, Some(std::path::PathBuf::from("config.toml")));
        assert_eq!(parsed.modes, vec![Mode::Gaps]);
        assert_eq!(parsed.iterations, 2);
        assert_eq!(parsed.concurrency, vec![2, 8]);
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

    const MOUNTS: &str = "\
/dev/sda1 / ext4 rw,relatime 0 0
//nas/abooks /mnt/nas/Audiobooks cifs rw,vers=3.1.1,cache=strict,actimeo=1 0 0
tmpfs /tmp tmpfs rw,nosuid 0 0";

    #[test]
    fn mount_lookup_picks_the_longest_matching_prefix() {
        let got = mount_for_path(
            MOUNTS,
            std::path::Path::new("/mnt/nas/Audiobooks/Author/Book"),
        );
        assert_eq!(
            got,
            Some((
                "cifs".to_string(),
                "rw,vers=3.1.1,cache=strict,actimeo=1".to_string()
            ))
        );
    }

    #[test]
    fn mount_lookup_falls_back_to_root() {
        let got = mount_for_path(MOUNTS, std::path::Path::new("/home/noah/abooks"));
        assert_eq!(got, Some(("ext4".to_string(), "rw,relatime".to_string())));
    }

    const NESTED_MOUNTS: &str = "\
/dev/sda1 / ext4 rw,relatime 0 0
//nas/media /mnt/nas cifs vers=3.0 0 0
//nas/abooks /mnt/nas/Audiobooks cifs vers=3.1.1 0 0";

    #[test]
    fn mount_lookup_prefers_the_deeper_of_two_nested_mounts() {
        // Both /mnt/nas and /mnt/nas/Audiobooks prefix the path, so the deeper (longer)
        // mount must win. The differing options expose a shorter-prefix regression.
        let got = mount_for_path(
            NESTED_MOUNTS,
            std::path::Path::new("/mnt/nas/Audiobooks/Author/Book"),
        );
        assert_eq!(got, Some(("cifs".to_string(), "vers=3.1.1".to_string())));
    }

    #[test]
    fn mount_lookup_returns_none_without_a_match() {
        assert_eq!(mount_for_path("", std::path::Path::new("/mnt/x")), None);
    }

    use std::fs;

    fn touch(path: &std::path::Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    fn bench_settings() -> ScanSettings {
        let audio: Vec<String> = [".mp3"].iter().map(|s| s.to_string()).collect();
        let ebook: Vec<String> = [".epub"].iter().map(|s| s.to_string()).collect();
        ScanSettings::compile(missing_ebooks::scanner::ScanInputs {
            audio_exts: &audio,
            ebook_exts: &ebook,
            excluded_dirs: &[],
            exclude_globs: &[],
        })
        .unwrap()
    }

    #[test]
    fn time_walk_full_counts_dirs_entries_gaps_and_audio() {
        let dir = tempfile::tempdir().unwrap();
        // A gap (audio, no cover) and a covered audiobook (audio + epub).
        touch(&dir.path().join("Gap/01.mp3"));
        touch(&dir.path().join("Gap/02.mp3"));
        touch(&dir.path().join("Covered/01.mp3"));
        touch(&dir.path().join("Covered/Book.epub"));
        let (_ms, counts) = time_walk(Mode::Full, dir.path(), &bench_settings());
        assert_eq!(counts.stats.dirs_visited, 3); // root, Gap, Covered
        assert_eq!(counts.stats.entries_seen, 6); // 2 subdirs + 2 + 2 files
        assert_eq!(counts.gaps, 1);
        assert_eq!(counts.audio_files, 3);
    }

    #[test]
    fn time_walk_gaps_counts_visited_dirs() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Gap/01.mp3"));
        touch(&dir.path().join("Covered/01.mp3"));
        touch(&dir.path().join("Covered/Book.epub"));
        let (_ms, counts) = time_walk(Mode::Gaps, dir.path(), &bench_settings());
        // The gaps walk reads root, Gap, and Covered. Covered's directory is read
        // before the cover is found.
        assert_eq!(counts.stats.dirs_visited, 3);
        assert_eq!(counts.gaps, 1);
        assert_eq!(counts.audio_files, 1);
    }

    #[test]
    fn time_reuse_walk_reuses_an_unchanged_tree() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Gap/01.mp3"));
        touch(&dir.path().join("Covered/01.mp3"));
        touch(&dir.path().join("Covered/Book.epub"));
        let settings = bench_settings();
        let mut index = scanner::DirIndex::new();
        // The first walk lists everything and fills the index. Nothing reused yet.
        let (_ms, build) = time_reuse_walk(dir.path(), &settings, &mut index);
        assert_eq!(build.stats.dirs_reused, 0);
        // The second walk on the unchanged tree reuses every directory, lists none.
        let (_ms, reuse) = time_reuse_walk(dir.path(), &settings, &mut index);
        assert_eq!(reuse.stats.dirs_visited, 3); // root, Gap, Covered
        assert_eq!(reuse.stats.dirs_reused, 3);
        assert_eq!(reuse.stats.entries_seen, 0);
        // One gap (Gap/), and both folders' audio is tallied from the cached facts.
        assert_eq!(reuse.gaps, 1);
        assert_eq!(reuse.audio_files, 2);
    }

    #[test]
    fn report_serializes_expected_keys() {
        let full_levels = vec![LevelReport {
            concurrency: 16,
            dirs_visited: 3,
            entries_seen: 9,
            gaps: 1,
            audio_files: 3,
            dirs_reused: None,
            cold: None,
            warm: phase_report(&[10.0, 20.0], 3),
        }];
        let incremental_levels = vec![LevelReport {
            concurrency: 16,
            dirs_visited: 3,
            entries_seen: 0,
            gaps: 1,
            audio_files: 3,
            dirs_reused: Some(3),
            cold: None,
            warm: phase_report(&[1.0, 2.0], 3),
        }];
        let mut modes = std::collections::BTreeMap::new();
        modes.insert(
            "full".to_string(),
            ModeReport {
                levels: full_levels,
            },
        );
        modes.insert(
            "incremental".to_string(),
            ModeReport {
                levels: incremental_levels,
            },
        );
        let report = Report {
            schema_version: SCHEMA_VERSION,
            tool: "scan_bench",
            label: "local".to_string(),
            host: "kessel".to_string(),
            kernel: "6.8.0".to_string(),
            unix_time: 1749700000,
            build_profile: "release",
            iterations: 2,
            drop_caches: false,
            roots: vec![RootReport {
                path: "/mnt/a".to_string(),
                fstype: "ext4".to_string(),
                mount_options: "rw".to_string(),
                modes,
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"schema_version\":3"));
        assert!(json.contains("\"tool\":\"scan_bench\""));
        assert!(json.contains("\"levels\""));
        assert!(json.contains("\"concurrency\":16"));
        assert!(json.contains("\"dirs_visited\":3"));
        assert!(json.contains("\"ms_per_dir\":5.0"));
        assert!(json.contains("\"cold\":null"));
        // The incremental level carries dirs_reused. The full and gaps modes omit it.
        assert!(json.contains("\"dirs_reused\":3"));
        let full_block = json.split("\"incremental\"").next().unwrap();
        assert!(!full_block.contains("dirs_reused"));
    }

    #[test]
    fn default_report_path_is_named_from_label_host_and_time() {
        let p = default_report_path("smb", "kessel", 1749700000);
        assert_eq!(
            p,
            std::path::PathBuf::from("scan-bench-smb-kessel-1749700000.json")
        );
    }

    #[test]
    fn read_trimmed_reads_and_trims_or_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value");
        fs::write(&path, "  kessel\n").unwrap();
        assert_eq!(read_trimmed(&path), Some("kessel".to_string()));
        assert_eq!(read_trimmed(&dir.path().join("missing")), None);
    }

    #[test]
    fn build_profile_reports_a_known_value() {
        assert!(matches!(build_profile(), "debug" | "release"));
    }
}
