# Benchmarks

`benches/scan_bench.rs` is a criterion bench that times the scanner across four groups: `scan_full` (full listing walk), `scan_gaps` (listing walk plus the reduce to flagged folders), `scan_warm` (reuse walk against a primed `DirIndex`), and `scan_concurrent` (five concurrent `RawViewStore::current()` callers against a fresh store). Default input is a synthetic tempdir at `total = 1000`, `depth = 3`, `fanout = 10`, `gap_rate = 0.5`, seeded per bench setup.

## Design

Four criterion groups mirror the four historical questions the bench answered: `scan_full` (full listing walk), `scan_gaps` (listing walk plus reduce to flagged folders), `scan_warm` (reuse walk against a primed `DirIndex`), and `scan_concurrent` (five concurrent `RawViewStore::current()` callers against a fresh store). Default input is a synthetic tempdir seeded via `missing_ebooks::synthetic::generate`; env vars point at a real filesystem or rehydrate the `example-nas` snapshot for backend probes. Criterion's `--baseline main` covers regression detection.

A small companion JSON per run records host, kernel, build profile, fstype, and mount options, since criterion's own output does not carry environmental context.

The 2026-06 schema-versioned JSON reports stay under `benchmarks/` as historical evidence for ADR-0019, ADR-0020, and ADR-0022; they no longer round-trip through the current bench binary. `cargo bench --bench scan_bench -- --baseline main` is the routine regression check; env-var overrides steer the same tool at real backends when a new question comes up.

## Regression check

Capture a baseline on `main` once:

```bash
cargo bench --bench scan_bench -- --save-baseline main
```

On a branch, compare against it:

```bash
cargo bench --bench scan_bench -- --baseline main
```

Criterion prints a `change: [-0.5% .. +0.3%]` delta per bench ID and flags meaningful regressions. `cargo bench --bench scan_bench -- --quick` runs the whole grid at reduced sample count for a smoke test.

## Backend probes

Point the bench at a real filesystem or a rehydrated snapshot via env vars. All are prefixed `MISSING_EBOOKS_SCAN_BENCH_`:

- `ROOT=/mnt/nas/Audiobooks` swaps the synthetic tempdir for a real path. Mutually exclusive with `SNAPSHOT`.
- `SNAPSHOT=1` rehydrates `tests/fixtures/example-nas/audiobooks.snapshot` into a tempdir and points the bench at that.
- `DROP_CACHES=1` (Linux only, sudo) runs `sync && echo 3 > /proc/sys/vm/drop_caches` before each `scan_full` and `scan_gaps` iteration. `scan_warm` and `scan_concurrent` ignore it. Pre-authenticate with `sudo -v` before the run.
- `CONCURRENCY=1,4,8,16,32` sweeps the rayon pool size for `scan_full`/`scan_gaps`/`scan_warm`. For `scan_concurrent`, the values are caller counts instead. Defaults are 16 threads for the first three and 5 callers for the last.
- `LABEL=smb` tags the companion JSON. Defaults to `unlabeled`.

A sample SMB run for the warm-reuse gate:

```bash
sudo -v
MISSING_EBOOKS_SCAN_BENCH_ROOT=/path/to/audiobooks \
MISSING_EBOOKS_SCAN_BENCH_DROP_CACHES=1 \
MISSING_EBOOKS_SCAN_BENCH_LABEL=smb \
cargo bench --bench scan_bench --release -- scan_warm
```

For the ADR-0019 concurrency curve:

```bash
sudo -v
MISSING_EBOOKS_SCAN_BENCH_ROOT=/mnt/pool/Audiobooks \
MISSING_EBOOKS_SCAN_BENCH_CONCURRENCY=1,4,8,16,32 \
MISSING_EBOOKS_SCAN_BENCH_DROP_CACHES=1 \
MISSING_EBOOKS_SCAN_BENCH_LABEL=local \
cargo bench --bench scan_bench --release -- scan_full
```

`scan_warm` assumes nothing else writes to the tree between iterations. Before a warm run against a real library, pause backups, indexers, and beets.

## Companion JSON

Each run writes `benchmarks/scan-context-<label>-<host>-<unix>.json`. It records host, kernel, build profile, whether `DROP_CACHES` was set, `input_source` (`synthetic`, `snapshot`, or `root`), and the root's fstype and mount options. Criterion owns the timings under `target/criterion/`; the companion carries only environmental context.

## Snapshot fixture

`tests/fixtures/example-nas/audiobooks.snapshot` is a frozen capture of one library's structure (about 900 directories, 7,900 files). Use it for relative comparisons within one machine via `MISSING_EBOOKS_SCAN_BENCH_SNAPSHOT=1`. Numbers from it are not comparable against the 2026-06 reports in this directory, which came from real mounts.

## 2026-06 sweep

Eighteen `scan-bench-*.json` files (schema v1 through v5) and three `cifs-*` text artifacts under this directory are the evidence base for ADR-0019, ADR-0020, and ADR-0022. They no longer round-trip through the bench binary; the per-run findings, fstab and `smb.conf` levers, and the result narratives live in [`EXPERIMENTS-2026-06.md`](EXPERIMENTS-2026-06.md).

## Render regression bench

`benches/render.rs` is a `criterion` bench that guards the ADR-0022 per-folder render claim. It seeds three sizes (1k, 10k, 50k folders) at one shape (`depth = 3`, `fanout` sized per row via `missing_ebooks::synthetic::synthetic_root_scan`), then times `render::page` and the per-section render across both view modes. The synthetic seeder is shared with `benches/scan_bench.rs`. The old `tree_bench` shape-sweep tool was removed once the render bench covered its regression role (`render::page` includes `tree::build`); it lives in git history.

The baseline/compare workflow matches `scan_bench`:

```bash
cargo bench --bench render -- --save-baseline main
cargo bench --bench render -- --baseline main
```

The per-folder column (under `Throughput`) is the figure ADR-0022 cites.

Both benches are excluded from `cargo test`. CI's `cargo clippy --all-targets` step in `.github/workflows/ci.yml` compile-checks `benches/render.rs` and `benches/scan_bench.rs` on every push, so a breaking change to their consumed surface fails CI before it reaches a developer's bench run. JSON reports under `target/criterion/` are not committed; this directory holds only long-lived scan-bench artifacts (the 2026-06 reports and per-run companion JSONs).
