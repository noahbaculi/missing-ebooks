# Benchmarks

`scan_bench` times the real scanner against the configured library roots, cold and warm, and writes a JSON report. The harness is `benches/scan_bench.rs`; run it with `cargo bench --bench scan_bench`. The analysis that drives the default `concurrency` value lives in [ADR-0019](../docs/adr/0019-scan-walk-parallel-sized-by-concurrency.md).

## Report shape

Each report is a single JSON object with a `runs` array. Each run carries a `concurrency`, a `mode` (`cold` or `warm`), per-iteration timings, and a derived `iteration_counts` block. Filenames are `scan-bench-<label>-<host>-<unix-ts>.json`; the timestamp orders the reports within a host.

## Running

Replace the path with your own library root:

    cargo bench --bench scan_bench -- --root /path/to/audiobooks --label local --drop-caches --concurrency 1,4,8,16,32

For the SMB sweep, mount the share first and point `--root` at the mount.

## Snapshot fixture

`tests/fixtures/example-nas/audiobooks.snapshot` is a frozen capture of one library's structure (about 900 directories, 7,900 files). Use it for relative comparisons within one machine. Numbers from it are not comparable against the reports in this directory, which came from real mounts.

## Run logs

The per-run findings, fstab and `smb.conf` levers, and the result narratives for the 2026-06 sweep live in [`EXPERIMENTS-2026-06.md`](EXPERIMENTS-2026-06.md).

## Render regression bench

`benches/render.rs` is a `criterion` bench that guards the ADR-0022 per-folder render claim. It seeds three sizes (1k, 10k, 50k folders) at one shape (`depth = 5`, `fanout = 10`, `gap_rate = 0.5`) via `missing_ebooks::synthetic::synthetic_root_scan`, then times `render_view` and the per-section OOB render across both view modes. The synthetic seeder is shared with `benches/tree_bench.rs`, which keeps its sweep-table role for ad hoc shape exploration (`cargo bench --bench tree_bench`). Audit `deep-dive/missing-ebooks-audit-2026-06-25.md` item #7 motivates the bench.

Capture a baseline on `main` once:

```bash
cargo bench --bench render -- --save-baseline main
```

On a branch, compare against it:

```bash
cargo bench --bench render -- --baseline main
```

Criterion prints a `change: [-0.5% .. +0.3%]` delta per bench ID and flags meaningful regressions in red. The per-folder column (under `Throughput`) is the figure ADR-0022 cites.

`cargo bench --bench render -- --quick` runs the whole grid in a few seconds at reduced sample count, for a smoke test or coarse iteration. JSON reports under `target/criterion/` are not committed; this directory holds only the long-lived `scan_bench` reports.

The bench is excluded from `cargo test`. CI's `cargo clippy --all-targets` step in `.github/workflows/ci.yml` compile-checks `benches/render.rs` on every push, so a breaking change to the renderer surface fails CI before it reaches a developer's bench run.
