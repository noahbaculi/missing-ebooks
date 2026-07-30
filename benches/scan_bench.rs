//! Criterion bench for the scan pipeline. Four groups, one per historical
//! scan mode: `scan_full`, `scan_gaps`, `scan_warm`, `scan_concurrent`.
//! Default input is a synthetic tempdir seeded via
//! `missing_ebooks::synthetic::generate`. Point at a real library with
//! `MISSING_EBOOKS_SCAN_BENCH_ROOT=/path`, or at a rehydrated
//! `example-nas` snapshot with `MISSING_EBOOKS_SCAN_BENCH_SNAPSHOT=1`.
//! `MISSING_EBOOKS_SCAN_BENCH_DROP_CACHES=1` (Linux only, sudo) flushes the
//! page cache before each Full or Gaps iteration.
//! `MISSING_EBOOKS_SCAN_BENCH_CONCURRENCY=1,4,8,16,32` sweeps thread counts
//! (or caller counts, for `scan_concurrent`). A companion JSON per run
//! lands at `benchmarks/scan-context-<label>-<host>-<unix>.json`.
//!
//! Regression check: `cargo bench --bench scan_bench -- --save-baseline main`
//! on `main`, then `cargo bench --bench scan_bench -- --baseline main` on a
//! branch. See `benchmarks/README.md`. Recorded in ADR-0035.

// `criterion_group!` and `criterion_main!` generate undocumented items.
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use missing_ebooks::config::Config;
use missing_ebooks::scanner::{self, DirIndex, ScanInputs, ScanSettings};
use missing_ebooks::state::RawViewStore;
use missing_ebooks::synthetic;
use serde::Serialize;
use tempfile::TempDir;

/// Default synthetic size, small enough to fit a four-group `--baseline main`
/// on a laptop in a couple of minutes and large enough to expose per-directory
/// scaling regressions.
const DEFAULT_TOTAL: usize = 1000;
const DEFAULT_DEPTH: usize = 3;
const DEFAULT_FANOUT: usize = 10;
const DEFAULT_GAP_RATE: f64 = 0.5;

/// Default rayon pool size for Full / Gaps / Warm groups.
const DEFAULT_THREADS: usize = 16;

/// Default caller count for the Concurrent group (five callers coalesce onto
/// one cold walk under single-flight; the invariant is pinned by `state::tests`).
const DEFAULT_CONCURRENT_CALLERS: usize = 5;

const ENV_ROOT: &str = "MISSING_EBOOKS_SCAN_BENCH_ROOT";
const ENV_SNAPSHOT: &str = "MISSING_EBOOKS_SCAN_BENCH_SNAPSHOT";
const ENV_DROP_CACHES: &str = "MISSING_EBOOKS_SCAN_BENCH_DROP_CACHES";
const ENV_CONCURRENCY: &str = "MISSING_EBOOKS_SCAN_BENCH_CONCURRENCY";
const ENV_LABEL: &str = "MISSING_EBOOKS_SCAN_BENCH_LABEL";

/// One resolved bench input. `tempdir` holds ownership when the source is
/// synthetic or a rehydrated snapshot; `None` when pointing at a real root
/// via `MISSING_EBOOKS_SCAN_BENCH_ROOT`.
struct BenchInput {
    root: PathBuf,
    settings: Arc<ScanSettings>,
    #[allow(dead_code)] // Kept alive so the tempdir is not dropped mid-bench.
    tempdir: Option<TempDir>,
    source: InputSource,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum InputSource {
    Synthetic,
    Snapshot,
    Root,
}

fn resolve_input() -> BenchInput {
    let has_root = std::env::var_os(ENV_ROOT).is_some();
    let has_snapshot = std::env::var_os(ENV_SNAPSHOT).is_some();
    assert!(
        !(has_root && has_snapshot),
        "{ENV_ROOT} and {ENV_SNAPSHOT} are mutually exclusive"
    );
    assert!(
        std::env::var_os(ENV_DROP_CACHES).is_none() || cfg!(target_os = "linux"),
        "{ENV_DROP_CACHES} is Linux-only"
    );

    let (root, tempdir, source) = if has_root {
        let raw = std::env::var(ENV_ROOT).expect("checked has_root above");
        (PathBuf::from(raw), None, InputSource::Root)
    } else if has_snapshot {
        let tmp = tempfile::tempdir().expect("tempdir for snapshot");
        rehydrate_snapshot(tmp.path());
        (tmp.path().to_path_buf(), Some(tmp), InputSource::Snapshot)
    } else {
        let tmp = tempfile::tempdir().expect("tempdir for synthetic");
        seed_synthetic(tmp.path());
        (tmp.path().to_path_buf(), Some(tmp), InputSource::Synthetic)
    };

    let audio = vec![".mp3".to_string(), ".m4b".to_string()];
    let ebook = vec![".epub".to_string(), ".pdf".to_string()];
    let settings = ScanSettings::compile(ScanInputs {
        audio_exts: &audio,
        ebook_exts: &ebook,
        excluded_dirs: &[],
        exclude_globs: &[],
    })
    .expect("compile scan settings");

    let input = BenchInput {
        root,
        settings: Arc::new(settings),
        tempdir,
        source,
    };
    write_companion_once(&input);
    input
}

/// Write the synthetic tree the seeder describes onto `root`. One empty file per
/// audio and ebook path; directories via `create_dir_all` on the parent.
fn seed_synthetic(root: &Path) {
    let folders = synthetic::generate(
        DEFAULT_TOTAL,
        DEFAULT_DEPTH,
        DEFAULT_FANOUT,
        DEFAULT_GAP_RATE,
    );
    for f in &folders {
        let dir = root.join(&f.rel_path);
        std::fs::create_dir_all(&dir).expect("create synthetic dir");
        for name in f.audio_files.iter() {
            std::fs::write(dir.join(name), b"").expect("write synthetic audio");
        }
        for name in f.cover_files.iter() {
            std::fs::write(dir.join(name), b"").expect("write synthetic ebook");
        }
    }
}

/// Rehydrate `tests/fixtures/example-nas/audiobooks.snapshot` into `root`. Each
/// snapshot line is a relative path; a trailing `/` is a directory, otherwise
/// an empty file with its parents.
fn rehydrate_snapshot(root: &Path) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/example-nas/audiobooks.snapshot");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
    for line in text.lines() {
        let trimmed = line.trim_end_matches('\n');
        if trimmed.is_empty() {
            continue;
        }
        let target = root.join(trimmed.trim_end_matches('/'));
        if trimmed.ends_with('/') {
            std::fs::create_dir_all(&target).expect("mkdir from snapshot");
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("mkdir parent from snapshot");
            }
            std::fs::write(&target, b"").expect("touch from snapshot");
        }
    }
}

#[derive(Debug, Serialize)]
struct Companion {
    tool: &'static str,
    label: String,
    host: String,
    kernel: String,
    unix_time: u64,
    build_profile: &'static str,
    drop_caches: bool,
    input_source: InputSource,
    roots: Vec<CompanionRoot>,
    criterion_output: &'static str,
}

#[derive(Debug, Serialize)]
struct CompanionRoot {
    path: String,
    fstype: String,
    mount_options: String,
}

static COMPANION_ONCE: Once = Once::new();

fn write_companion_once(input: &BenchInput) {
    COMPANION_ONCE.call_once(|| {
        let label = std::env::var(ENV_LABEL).unwrap_or_else(|_| "unlabeled".to_string());
        let host = read_trimmed(Path::new("/proc/sys/kernel/hostname"))
            .unwrap_or_else(|| "unknown".to_string());
        let kernel = read_trimmed(Path::new("/proc/sys/kernel/osrelease"))
            .unwrap_or_else(|| "unknown".to_string());
        let unix_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
        let canonical = std::fs::canonicalize(&input.root).unwrap_or_else(|_| input.root.clone());
        let (fstype, mount_options) = mount_for_path(&mounts, &canonical)
            .unwrap_or_else(|| ("unknown".to_string(), String::new()));

        let comp = Companion {
            tool: "scan_bench",
            label: label.clone(),
            host: host.clone(),
            kernel,
            unix_time,
            build_profile: build_profile(),
            drop_caches: std::env::var_os(ENV_DROP_CACHES).is_some(),
            input_source: input.source,
            roots: vec![CompanionRoot {
                path: canonical.display().to_string(),
                fstype,
                mount_options,
            }],
            criterion_output: "target/criterion/",
        };
        let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("benchmarks");
        let _ = std::fs::create_dir_all(&out_dir);
        let path = out_dir.join(format!("scan-context-{label}-{host}-{unix_time}.json"));
        match serde_json::to_string_pretty(&comp) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    eprintln!("scan_bench: could not write {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("scan_bench: could not encode companion: {e}"),
        }
    });
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Return the `(fstype, options)` of the mount whose mount point is the longest
/// prefix of `path`, given the text of `/proc/self/mounts`. Columns are device,
/// mount point, fstype, options. Ties broken by first occurrence.
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

/// Sudo-flush the Linux page cache, dentries, and inodes. Called before each
/// iteration of Full and Gaps when `MISSING_EBOOKS_SCAN_BENCH_DROP_CACHES=1`.
fn drop_caches() {
    let status = Command::new("sudo")
        .args(["sh", "-c", "sync && echo 3 > /proc/sys/vm/drop_caches"])
        .status()
        .expect("run sudo to drop caches");
    assert!(status.success(), "drop-caches exited with {status}");
}

/// Parse `MISSING_EBOOKS_SCAN_BENCH_CONCURRENCY=1,4,8,16,32`, or return the
/// single-element vec of `default` when unset. Panics on a bad value: the bench
/// is a developer tool and a malformed env var should fail loudly.
fn concurrency_levels(default: usize) -> Vec<usize> {
    match std::env::var(ENV_CONCURRENCY) {
        Ok(raw) => {
            let levels: Vec<usize> = raw
                .split(',')
                .map(|p| {
                    p.trim()
                        .parse()
                        .unwrap_or_else(|_| panic!("{ENV_CONCURRENCY}: {p:?} is not a number"))
                })
                .collect();
            assert!(
                levels.iter().all(|&n| n >= 1),
                "{ENV_CONCURRENCY} values must be >= 1"
            );
            assert!(!levels.is_empty(), "{ENV_CONCURRENCY} must not be empty");
            levels
        }
        Err(_) => vec![default],
    }
}

fn bench_scan_full(c: &mut Criterion) {
    let input = resolve_input();
    let drop_flag = std::env::var_os(ENV_DROP_CACHES).is_some();
    let mut group = c.benchmark_group("scan_full");
    for threads in concurrency_levels(DEFAULT_THREADS) {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("build rayon pool");
        // Capture dirs for Throughput from one warm-up walk.
        let (_, warm_stats) = pool.install(|| {
            let index = DirIndex::new();
            scanner::scan_warm(&input.root, &input.settings, &index)
        });
        group.throughput(Throughput::Elements(warm_stats.dirs_visited as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &(&input, &pool, drop_flag),
            |b, (input, pool, drop_flag)| {
                b.iter_batched(
                    DirIndex::new,
                    |index| {
                        if *drop_flag {
                            drop_caches();
                        }
                        pool.install(|| scanner::scan_warm(&input.root, &input.settings, &index))
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_scan_gaps(c: &mut Criterion) {
    let input = resolve_input();
    let drop_flag = std::env::var_os(ENV_DROP_CACHES).is_some();
    let mut group = c.benchmark_group("scan_gaps");
    for threads in concurrency_levels(DEFAULT_THREADS) {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("build rayon pool");
        let (folders, warm_stats) = pool.install(|| {
            let index = DirIndex::new();
            scanner::scan_warm(&input.root, &input.settings, &index)
        });
        let _ = scanner::reduce_to_flagged(&folders);
        group.throughput(Throughput::Elements(warm_stats.dirs_visited as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &(&input, &pool, drop_flag),
            |b, (input, pool, drop_flag)| {
                b.iter_batched(
                    DirIndex::new,
                    |index| {
                        if *drop_flag {
                            drop_caches();
                        }
                        pool.install(|| {
                            let (folders, _) =
                                scanner::scan_warm(&input.root, &input.settings, &index);
                            scanner::reduce_to_flagged(&folders)
                        })
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_scan_warm(c: &mut Criterion) {
    let input = resolve_input();
    let mut group = c.benchmark_group("scan_warm");
    for threads in concurrency_levels(DEFAULT_THREADS) {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("build rayon pool");
        let index = DirIndex::new();
        // Prime the index with one discarded listing walk so the timed closure
        // measures the reuse walk against a hot index.
        let (_, warm_stats) =
            pool.install(|| scanner::scan_warm(&input.root, &input.settings, &index));
        group.throughput(Throughput::Elements(warm_stats.dirs_visited as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &(&input, &pool, &index),
            |b, (input, pool, index)| {
                b.iter(|| pool.install(|| scanner::scan_warm(&input.root, &input.settings, index)));
            },
        );
    }
    group.finish();
}

fn bench_scan_concurrent(c: &mut Criterion) {
    let input = resolve_input();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let cfg = Arc::new(Config {
        library_roots: vec![input.root.clone()],
        ttl_seconds: 600,
        ..Default::default()
    });
    let mut group = c.benchmark_group("scan_concurrent");
    for callers in concurrency_levels(DEFAULT_CONCURRENT_CALLERS) {
        group.bench_with_input(
            BenchmarkId::from_parameter(callers),
            &callers,
            |b, &callers| {
                b.iter_batched(
                    || {
                        let dir_indices = vec![Arc::new(DirIndex::new())];
                        Arc::new(RawViewStore::new(
                            Arc::clone(&cfg),
                            Arc::clone(&input.settings),
                            dir_indices,
                            Some(Duration::from_secs(600)),
                        ))
                    },
                    |store| {
                        runtime.block_on(async {
                            let handles: Vec<_> = (0..callers)
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
                        });
                    },
                    criterion::BatchSize::PerIteration,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_scan_full,
    bench_scan_gaps,
    bench_scan_warm,
    bench_scan_concurrent
);
criterion_main!(benches);
