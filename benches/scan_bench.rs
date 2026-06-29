//! Read-only benchmark: time the real scanner against the configured library
//! roots, local disk versus an SMB (CIFS) mount.
//!
//! `cargo bench --bench scan_bench -- --config config.toml --label smb --drop-caches`
//! loads the real `Config`, compiles `ScanSettings`, and times `scanner::scan_warm`
//! per root, in cold and warm
//! cache conditions, then saves a JSON report. The walks only read directory
//! entries and names; nothing here writes to the library. The single privileged
//! action is the optional `--drop-caches` page-cache flush on Linux.
//!
//! On a prod box: build and run with `--release` so the timings reflect the
//! shipped binary, not a debug build. `--drop-caches` flushes the page cache for
//! the cold runs and needs root on Linux, so run that invocation under `sudo`;
//! omit the flag for warm-only numbers. With no Rust toolchain on the box, build
//! `target/release/scan_bench` on a machine of matching OS and arch and
//! copy the binary over. Point it at the real mounts with `--config config.toml`,
//! repeated `--root PATH`, or `MISSING_EBOOKS_LIBRARY_ROOTS`.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use clap::Parser;
use missing_ebooks::config::Config;
use missing_ebooks::scanner::{self, DirIndex, ScanSettings, WalkStats};
use missing_ebooks::state::RawViewStore;
use missing_ebooks::tree;
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
        f64::midpoint(v[n / 2 - 1], v[n / 2])
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

/// Per-iteration walk counts for the warm mode, one row per measured
/// iteration in `iterations_ms` order. A row whose `dirs_reused` or `entries_seen`
/// drifts mid-run flags an external write to the supposedly unchanged tree.
#[derive(Debug, Serialize, Clone, Copy)]
struct IterCounts {
    /// Directories the walk would have read, including the ones served from the index.
    dirs_visited: usize,
    /// Directories served from the index without a listing.
    dirs_reused: usize,
    /// Directory entries iterated. Zero on a fully reused walk.
    entries_seen: usize,
}

/// One cache condition's timing summary for one root and mode.
#[derive(Debug, Serialize)]
struct PhaseReport {
    /// Each measured iteration's wall-clock, in milliseconds, in run order.
    iterations_ms: Vec<f64>,
    /// Per-iteration walk counts, one row per `iterations_ms` sample. Recorded
    /// for the warm mode. Empty for the listing walks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    iteration_counts: Vec<IterCounts>,
    median_ms: f64,
    min_ms: f64,
    max_ms: f64,
    /// Median divided by directories visited. `None` only when no directory was walked.
    ms_per_dir: Option<f64>,
}

/// Summarize one phase's samples. `dirs` is the directory count for the median's
/// per-directory figure. `counts` is the per-iteration counts for the warm
/// mode, empty for listing walks.
fn phase_report(samples: &[f64], dirs: usize, counts: Vec<IterCounts>) -> PhaseReport {
    let median_ms = median(samples);
    PhaseReport {
        iterations_ms: samples.to_vec(),
        iteration_counts: counts,
        median_ms,
        min_ms: min_of(samples),
        max_ms: max_of(samples),
        ms_per_dir: per_dir_ms(median_ms, dirs),
    }
}

/// Which walk to time. `Gaps` reduces the full walk to flagged folders, `Full`
/// records every directory, and `Warm` reuses unchanged directories via the
/// mtime index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gaps,
    Full,
    Warm,
    /// Five concurrent `RawViewStore::current()` calls against a fresh store
    /// over the configured roots. With single-flight cold builds the wall time
    /// tracks one scan. Without it, five. The unit invariant (one rebuild
    /// per cold burst) is pinned by `state::tests`. This is the wall-time
    /// view for the maintainer.
    Concurrent,
}

impl Mode {
    /// The lowercase name used in stdout and as the report's mode key.
    fn label(self) -> &'static str {
        match self {
            Mode::Gaps => "gaps",
            Mode::Full => "full",
            Mode::Warm => "warm",
            Mode::Concurrent => "concurrent",
        }
    }
}

/// Map `--mode`: one keyword or a comma-separated list, e.g. `full,warm`.
/// `every` expands to all listing-walk modes (full, gaps, warm). `concurrent`
/// is opt-in because it builds a tokio runtime and a full `RawViewStore`,
/// which the other modes do not need. Duplicates collapse, first occurrence
/// wins, so the listed order is the report order. `all` is not a keyword:
/// older reports key the full walk as `all`, so it stays a report value only,
/// never a live selector.
fn parse_modes(value: &str) -> Result<Vec<Mode>, String> {
    let mut modes = Vec::new();
    for part in value.split(',') {
        let expanded = match part.trim() {
            "gaps" => vec![Mode::Gaps],
            "full" => vec![Mode::Full],
            "warm" => vec![Mode::Warm],
            "concurrent" => vec![Mode::Concurrent],
            "every" => vec![Mode::Full, Mode::Gaps, Mode::Warm],
            other => {
                return Err(format!(
                    "--mode: {other:?} must be full, gaps, warm, concurrent, or every"
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

/// scan_bench CLI surface. Mirrors `bin/explore.rs`: a clap derive struct with
/// the same flag set the hand-rolled parser carried. `--mode` and
/// `--concurrency` stay raw strings so the comma-list and `every` expansion
/// live in `parse_modes`/`parse_concurrency`, called in `main`.
#[derive(clap::Parser, Debug)]
#[command(
    name = "scan_bench",
    version,
    about = "Time the real scanner (read-only) against each library root and save a JSON report.",
    after_help = "Roots come from --root, --config, or MISSING_EBOOKS_LIBRARY_ROOTS.\n\
        --mode is comma-separated: full, gaps, warm, concurrent, or every (default \
        every, the listing-walk modes). `concurrent` is opt-in: it builds a tokio \
        runtime and times five simultaneous RawViewStore::current() calls.\n\
        --drop-caches sudo-flushes the Linux page cache before each cold run."
)]
struct Cli {
    /// Load the real config.toml (extensions, exclusions, roots).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Benchmark this exact path; repeatable; replaces config roots.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Measured runs per phase.
    #[arg(long, default_value_t = 5, value_parser = parse_iterations)]
    iterations: usize,
    /// Comma-separated: full, gaps, warm, concurrent, or every.
    #[arg(long = "mode", default_value = "every")]
    mode: String,
    /// Thread counts to sweep, comma-separated, e.g. 1,4,8,16.
    #[arg(long, default_value = "16")]
    concurrency: String,
    /// Linux: sudo-flush the page cache before each cold run.
    #[arg(long)]
    drop_caches: bool,
    /// Tag stdout and the report (e.g. local, smb).
    #[arg(long)]
    label: Option<String>,
    /// Report path (default scan-bench-<label>-<host>-<time>.json).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Do not write the report file.
    #[arg(long)]
    no_save: bool,
    /// Absorbed: cargo bench passes this through to the binary.
    #[arg(long, hide = true)]
    bench: bool,
}

/// Return the `(fstype, options)` of the mount whose mount point is the longest
/// prefix of `path`, given the text of `/proc/self/mounts`. Columns are device,
/// mount point, fstype, options. A shorter line is skipped. Names the filesystem
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
/// and entry totals from the walk itself. `gaps` and `audio_files` are derived
/// from the result after the clock stops. `tree_build_ms` is the wall time of the
/// per-mode render (`reduce_to_flagged` then `tree::build` for gaps, direct
/// `tree::build` for full, warm matches its underlying mode), timed
/// after the walk so it does not inflate the walk number.
struct WalkCounts {
    stats: WalkStats,
    gaps: usize,
    audio_files: usize,
    tree_build_ms: f64,
}

/// The report schema version, bumped when the JSON shape changes so a directory of
/// mixed-vintage reports stays parseable. Schema 3 adds the `incremental` mode (renamed
/// to `warm` in a later schema bump): a
/// single-level entry carrying `dirs_reused`, absent on the `full` and `gaps` modes.
/// Schema 4 adds `tree_build_ms` on every level. Schema 5 adds `iteration_counts`
/// on the warm phases.
const SCHEMA_VERSION: u32 = 5;

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
/// timings keyed by mode label (`full`, `gaps`, `warm`) for a stable order.
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
/// `--drop-caches` was off. `dirs_reused` is `Some` only for the warm mode,
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
    /// Wall time of the per-mode render that runs after the walk, in milliseconds.
    /// Zero in vintage reports (schema_version < 4); always present from v4 on.
    tree_build_ms: f64,
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

/// Measure warm-scan reuse and fold it into the JSON report as a single-level
/// `warm` mode. Build the index with one full listing walk (discarded), then
/// time reuse walks against it. `--drop-caches` adds a cold phase that drops the
/// client cache before each walk, so every directory stat is a real round trip, the
/// honest figure over a network mount. The warm phase shows the cache-hot case.
/// Concurrency is inert over SMB for the reuse walk, so this runs at one `threads`
/// rather than sweeping it, recording that as the level's concurrency. The reused-vs-
/// listed split and the per-phase timings also print live as the run proceeds.
fn run_warm(
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

    let index = scanner::DirIndex::new();
    // Build the index: this first walk lists everything, so its timing is discarded.
    let (_ms, mut last) = pool.install(|| time_reuse_walk(root, settings, &index));

    let cold = if drop_caches_enabled {
        let mut samples = Vec::with_capacity(iterations);
        let mut iter_counts = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            drop_caches()?;
            let (ms, counts) = pool.install(|| time_reuse_walk(root, settings, &index));
            samples.push(ms);
            iter_counts.push(iter_counts_from(&counts));
            last = counts;
        }
        Some(phase_report(&samples, last.stats.dirs_visited, iter_counts))
    } else {
        None
    };

    // Warm phase: one discarded warmup, then measured reuse walks with no drop.
    let _ = pool.install(|| time_reuse_walk(root, settings, &index));
    let mut samples = Vec::with_capacity(iterations);
    let mut iter_counts = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let (ms, counts) = pool.install(|| time_reuse_walk(root, settings, &index));
        samples.push(ms);
        iter_counts.push(iter_counts_from(&counts));
        last = counts;
    }
    let warm = phase_report(&samples, last.stats.dirs_visited, iter_counts);

    println!(
        "  mode=warm  concurrency={threads}  dirs_visited={}  dirs_reused={}  \
         entries_seen={}  tree_build_ms={}  (index {} dirs)",
        last.stats.dirs_visited,
        last.stats.dirs_reused,
        last.stats.entries_seen,
        last.tree_build_ms,
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
            tree_build_ms: last.tree_build_ms,
            cold,
            warm,
        }],
    })
}

/// Concurrent cold-cache scenario: fire `concurrency` simultaneous
/// `RawViewStore::current()` calls and report the wall clock to last-done.
/// With single-flight cold builds the callers coalesce onto one walk and the
/// wall time tracks one scan. Without it, the locked-across-await cache
/// serializes them and the wall time tracks `concurrency` scans. The unit
/// invariant (one rebuild per cold burst) is pinned by `state::tests`. This
/// is the wall-time view for the maintainer.
fn run_concurrent(
    root: &Path,
    settings: &Arc<ScanSettings>,
    iterations: usize,
    concurrency: usize,
) -> Result<ModeReport, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("could not build a tokio runtime: {e}"))?;

    // The store ttl is irrelevant for cold-burst measurement: every
    // iteration constructs a fresh store so every call hits a cold slot.
    let cfg = Config {
        library_roots: vec![root.to_path_buf()],
        ttl_seconds: 600,
        ..Default::default()
    };
    let config = Arc::new(cfg);
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let dir_indices = vec![Arc::new(DirIndex::new())];
        let store = Arc::new(RawViewStore::new(
            Arc::clone(&config),
            Arc::clone(settings),
            dir_indices,
            Some(Duration::from_secs(600)),
        ));
        let started = Instant::now();
        let elapsed_ms = runtime.block_on(async {
            let handles: Vec<_> = (0..concurrency)
                .map(|_| {
                    let s = Arc::clone(&store);
                    tokio::spawn(async move {
                        let _ = s.current().await;
                    })
                })
                .collect();
            for h in handles {
                let _ = h.await;
            }
            round3(started.elapsed().as_secs_f64() * 1000.0)
        });
        samples.push(elapsed_ms);
    }

    // No per-iteration walk counts: the wall-clock burst is the measurement
    // and the rebuild-count invariant is the unit test.
    let warm = phase_report(&samples, 0, Vec::new());
    println!(
        "  mode=concurrent  concurrency={concurrency}  iterations={iterations}  \
         wall_median_ms={}  (one rebuild per burst is pinned by state::tests)",
        warm.median_ms
    );

    Ok(ModeReport {
        levels: vec![LevelReport {
            concurrency,
            dirs_visited: 0,
            entries_seen: 0,
            gaps: 0,
            audio_files: 0,
            dirs_reused: None,
            tree_build_ms: 0.0,
            cold: None,
            warm,
        }],
    })
}

/// Pull the per-iteration counts out of a `WalkCounts`.
fn iter_counts_from(c: &WalkCounts) -> IterCounts {
    IterCounts {
        dirs_visited: c.stats.dirs_visited,
        dirs_reused: c.stats.dirs_reused,
        entries_seen: c.stats.entries_seen,
    }
}

/// Time one reuse walk against the prebuilt `index`, returning its wall-clock in
/// milliseconds and the walk's counts. The gap and audio tallies are derived after
/// the clock stops, like `time_walk`, so the in-memory reduce never inflates the
/// measured wall-clock. On an unchanged tree every directory is reused and no entry
/// is listed.
fn time_reuse_walk(
    root: &Path,
    settings: &ScanSettings,
    index: &scanner::DirIndex,
) -> (f64, WalkCounts) {
    let walk_start = Instant::now();
    let (folders, stats) = scanner::scan_warm(root, settings, index);
    let walk_ms = round3(walk_start.elapsed().as_secs_f64() * 1000.0);
    let flagged = scanner::reduce_to_flagged(&folders);
    let gaps = flagged.len();
    let audio_files: usize = flagged.iter().map(|f| f.audio_files.len()).sum();
    drop(flagged);
    let scan = scanner::RootScan::Walked {
        canonical_path: root.to_path_buf(),
        folders,
    };
    let render_start = Instant::now();
    let state = tree::build(&scan, tree::ViewMode::GapsOnly);
    let tree_build_ms = round3(render_start.elapsed().as_secs_f64() * 1000.0);
    std::hint::black_box(&state);
    (
        walk_ms,
        WalkCounts {
            stats,
            gaps,
            audio_files,
            tree_build_ms,
        },
    )
}

/// Time one read-only walk and the per-mode render that follows it. Only the walk
/// sits inside the first `Instant`. The render is timed separately, so the walk
/// number stays comparable across schema versions. The `gaps` and `audio_files`
/// tallies are derived after both clocks stop.
fn time_walk(mode: Mode, root: &Path, settings: &ScanSettings) -> (f64, WalkCounts) {
    match mode {
        Mode::Full => {
            let walk_start = Instant::now();
            let (folders, stats) = scanner::scan_warm(root, settings, &scanner::DirIndex::new());
            let walk_ms = round3(walk_start.elapsed().as_secs_f64() * 1000.0);
            let gaps = folders
                .iter()
                .filter(|f| f.directly_holds_audio && f.missing_ebook)
                .count();
            let audio_files = folders.iter().map(|f| f.audio_files.len()).sum();
            let scan = scanner::RootScan::Walked {
                canonical_path: root.to_path_buf(),
                folders,
            };
            let render_start = Instant::now();
            let state = tree::build(&scan, tree::ViewMode::All);
            let tree_build_ms = round3(render_start.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(&state);
            (
                walk_ms,
                WalkCounts {
                    stats,
                    gaps,
                    audio_files,
                    tree_build_ms,
                },
            )
        }
        Mode::Gaps => {
            let walk_start = Instant::now();
            let (folders, stats) = scanner::scan_warm(root, settings, &scanner::DirIndex::new());
            let walk_ms = round3(walk_start.elapsed().as_secs_f64() * 1000.0);
            let flagged = scanner::reduce_to_flagged(&folders);
            let gaps = flagged.len();
            let audio_files: usize = flagged.iter().map(|f| f.audio_files.len()).sum();
            drop(flagged);
            let scan = scanner::RootScan::Walked {
                canonical_path: root.to_path_buf(),
                folders,
            };
            let render_start = Instant::now();
            let state = tree::build(&scan, tree::ViewMode::GapsOnly);
            let tree_build_ms = round3(render_start.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(&state);
            (
                walk_ms,
                WalkCounts {
                    stats,
                    gaps,
                    audio_files,
                    tree_build_ms,
                },
            )
        }
        // Warm is routed to run_warm before any phase calls this.
        Mode::Warm => unreachable!("warm mode does not use time_walk"),
        // Concurrent is routed to run_concurrent before any phase calls this.
        Mode::Concurrent => unreachable!("concurrent mode does not use time_walk"),
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
    Ok((
        phase_report(&samples, counts.stats.dirs_visited, Vec::new()),
        counts,
    ))
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
    (
        phase_report(&samples, counts.stats.dirs_visited, Vec::new()),
        counts,
    )
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
fn resolve_config(cli: &Cli) -> Result<Config, String> {
    let mut config = match cli.config.as_deref() {
        Some(path) => Config::load(Some(path)).map_err(|e| e.to_string())?,
        None if cli.roots.is_empty() => Config::load(None).map_err(|e| e.to_string())?,
        None => Config::default(),
    };
    if !cli.roots.is_empty() {
        config.library_roots.clone_from(&cli.roots);
    }
    Ok(config)
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Scanner warnings (an unreadable root, a covering root) should be visible.
    tracing_subscriber::fmt::init();

    let modes = match parse_modes(&cli.mode) {
        Ok(modes) => modes,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let concurrency = match parse_concurrency(&cli.concurrency) {
        Ok(levels) => levels,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    let config = match resolve_config(&cli) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let settings = match ScanSettings::compile(config.scan_inputs()) {
        Ok(settings) => Arc::new(settings),
        Err(err) => {
            eprintln!("invalid scan settings: {err}");
            return ExitCode::FAILURE;
        }
    };

    let host = hostname();
    let kernel = kernel_release();
    let profile = build_profile();
    let unix = unix_time();
    let label = cli.label.clone().unwrap_or_else(|| "unlabeled".to_string());
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();

    println!(
        "scan_bench [{label}] host={host} kernel={kernel} profile={profile} \
         iterations={} drop_caches={}",
        cli.iterations, cli.drop_caches
    );
    if profile == "debug" {
        println!("  note: build with --release for an honest local baseline");
    }
    if cli.drop_caches {
        println!("  note: cold means client-side cold; the SMB server may still cache the tree");
    }
    if modes.contains(&Mode::Warm) {
        println!(
            "  note: warm mode assumes nothing else writes to the tree between walks; \
             pause backups, indexers, and beets"
        );
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

        let mut mode_reports = BTreeMap::new();
        for &mode in &modes {
            // Warm is a focused reuse measurement, outside the concurrency
            // sweep: it runs once at the top swept thread count (16 by default),
            // since concurrency is inert over SMB for the reuse walk.
            if mode == Mode::Warm {
                let threads = concurrency.iter().copied().max().unwrap_or(16);
                match run_warm(
                    &canonical,
                    &settings,
                    cli.iterations,
                    threads,
                    cli.drop_caches,
                ) {
                    Ok(report) => {
                        mode_reports.insert(mode.label().to_string(), report);
                    }
                    Err(message) => {
                        eprintln!("error during warm run: {message}");
                        return ExitCode::FAILURE;
                    }
                }
                continue;
            }
            if mode == Mode::Concurrent {
                // Run once at the top swept concurrency level (default 5), so a
                // single config produces a single burst measurement per root.
                let threads = concurrency.iter().copied().max().unwrap_or(5);
                match run_concurrent(&canonical, &settings, cli.iterations, threads) {
                    Ok(report) => {
                        mode_reports.insert(mode.label().to_string(), report);
                    }
                    Err(message) => {
                        eprintln!("error during concurrent run: {message}");
                        return ExitCode::FAILURE;
                    }
                }
                continue;
            }
            let mut levels = Vec::new();
            for &threads in &concurrency {
                let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
                    Ok(pool) => pool,
                    Err(err) => {
                        eprintln!("error: could not build a {threads}-thread pool: {err}");
                        return ExitCode::FAILURE;
                    }
                };
                // Run the phases inside the pool so the scanner's parallel walk
                // uses exactly these `threads` workers for this sweep entry.
                let cold = if cli.drop_caches {
                    match pool.install(|| cold_phase(mode, &canonical, &settings, cli.iterations)) {
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
                    pool.install(|| warm_phase(mode, &canonical, &settings, cli.iterations));

                println!(
                    "  mode={} concurrency={}  dirs_visited={}  entries_seen={}  gaps={}  \
                     audio_files={}  tree_build_ms={}",
                    mode.label(),
                    threads,
                    counts.stats.dirs_visited,
                    counts.stats.entries_seen,
                    counts.gaps,
                    counts.audio_files,
                    counts.tree_build_ms
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
                    tree_build_ms: counts.tree_build_ms,
                    cold,
                    warm,
                });
            }
            mode_reports.insert(mode.label().to_string(), ModeReport { levels });
        }

        roots.push(RootReport {
            path: canonical.display().to_string(),
            fstype,
            mount_options: options,
            modes: mode_reports,
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
        iterations: cli.iterations,
        drop_caches: cli.drop_caches,
        roots,
    };

    if cli.no_save {
        println!("\n--no-save: report not written");
    } else {
        let path = cli
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
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;

    #[test]
    fn phase_report_aggregates_samples_with_per_dir() {
        let p = phase_report(&[10.0, 20.0, 30.0], 10, Vec::new());
        assert_eq!(p.iterations_ms, vec![10.0, 20.0, 30.0]);
        assert_eq!(p.median_ms, 20.0);
        assert_eq!(p.min_ms, 10.0);
        assert_eq!(p.max_ms, 30.0);
        assert_eq!(p.ms_per_dir, Some(2.0));
        assert!(p.iteration_counts.is_empty());
    }

    #[test]
    fn phase_report_leaves_per_dir_none_with_zero_dirs() {
        let p = phase_report(&[10.0, 20.0], 0, Vec::new());
        assert_eq!(p.median_ms, 15.0);
        assert_eq!(p.ms_per_dir, None);
    }

    #[test]
    fn phase_report_carries_iteration_counts_when_provided() {
        // The values land in the report verbatim, not aggregated, so the JSON shows
        // a row that drifted.
        let counts = vec![
            IterCounts {
                dirs_visited: 100,
                dirs_reused: 100,
                entries_seen: 0,
            },
            IterCounts {
                dirs_visited: 100,
                dirs_reused: 99,
                entries_seen: 12,
            },
        ];
        let p = phase_report(&[10.0, 20.0], 100, counts);
        assert_eq!(p.iteration_counts.len(), 2);
        assert_eq!(p.iteration_counts[0].dirs_reused, 100);
        assert_eq!(p.iteration_counts[1].dirs_reused, 99);
        assert_eq!(p.iteration_counts[1].entries_seen, 12);
        // The field serializes when non-empty so a reader can spot the drift.
        let json = serde_json::to_value(&p).unwrap();
        assert!(json.get("iteration_counts").is_some());
    }

    #[test]
    fn phase_report_skips_iteration_counts_when_empty() {
        // Listing walks (full, gaps) walk the whole tree every iteration by
        // definition, so the field is omitted to keep their JSON compact.
        let p = phase_report(&[10.0], 1, Vec::new());
        let json = serde_json::to_value(&p).unwrap();
        assert!(json.get("iteration_counts").is_none());
    }

    #[test]
    fn level_report_records_tree_build_ms() {
        let p = phase_report(&[10.0, 20.0, 30.0], 10, Vec::new());
        let level = LevelReport {
            concurrency: 16,
            dirs_visited: 10,
            entries_seen: 100,
            gaps: 1,
            audio_files: 5,
            dirs_reused: None,
            tree_build_ms: 0.42,
            cold: None,
            warm: p,
        };
        let json = serde_json::to_value(&level).unwrap();
        assert_eq!(json["tree_build_ms"], serde_json::json!(0.42));
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
        let audio: Vec<String> = [".mp3"].iter().map(ToString::to_string).collect();
        let ebook: Vec<String> = [".epub"].iter().map(ToString::to_string).collect();
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
        let index = scanner::DirIndex::new();
        // The first walk lists everything and fills the index. Nothing reused yet.
        let (_ms, build) = time_reuse_walk(dir.path(), &settings, &index);
        assert_eq!(build.stats.dirs_reused, 0);
        // The second walk on the unchanged tree reuses every directory, lists none.
        let (_ms, reuse) = time_reuse_walk(dir.path(), &settings, &index);
        assert_eq!(reuse.stats.dirs_visited, 3); // root, Gap, Covered
        assert_eq!(reuse.stats.dirs_reused, 3);
        assert_eq!(reuse.stats.entries_seen, 0);
        // One gap (Gap/); the audio tally is the gaps-render sum, so only Gap's
        // file counts even though Covered's audio sits in the cached facts.
        assert_eq!(reuse.gaps, 1);
        assert_eq!(reuse.audio_files, 1);
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
            tree_build_ms: 0.0,
            cold: None,
            warm: phase_report(&[10.0, 20.0], 3, Vec::new()),
        }];
        let warm_levels = vec![LevelReport {
            concurrency: 16,
            dirs_visited: 3,
            entries_seen: 0,
            gaps: 1,
            audio_files: 3,
            dirs_reused: Some(3),
            tree_build_ms: 0.0,
            cold: None,
            warm: phase_report(
                &[1.0, 2.0],
                3,
                vec![
                    IterCounts {
                        dirs_visited: 3,
                        dirs_reused: 3,
                        entries_seen: 0,
                    },
                    IterCounts {
                        dirs_visited: 3,
                        dirs_reused: 3,
                        entries_seen: 0,
                    },
                ],
            ),
        }];
        let mut modes = std::collections::BTreeMap::new();
        modes.insert(
            "full".to_string(),
            ModeReport {
                levels: full_levels,
            },
        );
        modes.insert(
            "warm".to_string(),
            ModeReport {
                levels: warm_levels,
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
        assert!(json.contains("\"schema_version\":5"));
        assert!(json.contains("\"tool\":\"scan_bench\""));
        assert!(json.contains("\"levels\""));
        assert!(json.contains("\"concurrency\":16"));
        assert!(json.contains("\"dirs_visited\":3"));
        assert!(json.contains("\"ms_per_dir\":5.0"));
        assert!(json.contains("\"cold\":null"));
        // The warm level carries dirs_reused. The full and gaps modes omit it.
        assert!(json.contains("\"dirs_reused\":3"));
        // Anchor on the mode-map key (`"warm":{"levels"`) rather than the bare
        // string, since `warm` is also a per-level phase name in PhaseReport.
        let full_block = json.split("\"warm\":{\"levels\"").next().unwrap();
        assert!(!full_block.contains("dirs_reused"));
        // iteration_counts is present on the warm phase only.
        let warm_block = json.split("\"warm\":{\"levels\"").nth(1).unwrap();
        assert!(warm_block.contains("\"iteration_counts\""));
        assert!(!full_block.contains("iteration_counts"));
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
