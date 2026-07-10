# ADR-0035: The scan bench is a criterion bench, not a schema-versioned JSON writer

Date: 2026-07-06.

## Context

`benches/scan_bench.rs` grew into 1284 lines over the 2026-06 SMB investigation: five schema versions, a hand-rolled clap CLI, a `/proc/self/mounts` parser, per-iteration walk-count canaries, and a comma-list mode selector. It answered the questions ADR-0019, ADR-0020, ADR-0022, and ADR-0023 turn on, and has run maybe a handful of times since. Two use cases survive: catching scan-side performance regressions on scan-touching PRs, and probing new storage backends when they appear.

## Decision

The bench is a criterion bench in the same shape as `benches/render.rs`. Four groups, one per historical mode (`scan_full`, `scan_gaps`, `scan_warm`, `scan_concurrent`). Default input is a synthetic tempdir seeded via `missing_ebooks::synthetic::generate`; env vars point at a real filesystem or rehydrate the `example-nas` snapshot for backend probes. Criterion's `--baseline main` covers regression detection. A small companion JSON per run records host, kernel, build profile, fstype, and mount options, since criterion's own output does not carry environmental context.

## Consequences

The 2026-06 JSON reports stay in `benchmarks/` as historical evidence for the ADRs that cite them; they no longer round-trip through the current bench binary, which is fine because nothing parsed them anyway. The schema-version constant, custom JSON writer, `IterCounts` canary, comma-list mode selector, and concurrency-sweep loop are gone; the four mode implementations, `mount_for_path`, and the sudo drop-caches shell-out survive. Bench LOC drops from about 1284 to about 300. `cargo bench --bench scan_bench -- --baseline main` becomes the routine regression check; env-var overrides steer the same tool at real backends when a new question comes up.

## Related

- ADR-0019 (scan-walk parallel sized by concurrency): the local-vs-16-thread numbers it cites still live in the schema-v3 reports under `benchmarks/`; the reproduction workflow is documented in `benchmarks/README.md`.
- ADR-0022 (raw cache + render per request): its per-folder gate is now covered by `benches/render.rs` and the `scan_full` / `scan_gaps` groups of the new criterion bench.
- ADR-0023 (warm-reuse gate): reproduction is `cargo bench --bench scan_bench -- scan_warm` with `MISSING_EBOOKS_SCAN_BENCH_ROOT` and `MISSING_EBOOKS_SCAN_BENCH_DROP_CACHES=1`.
