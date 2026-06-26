# missing-ebooks — Pre-release audit (2026-06-26)

Scope: full repo. Code, docs, build, deps, security, public surface, repo hygiene. Solo-developer project preparing its first public release. Supersedes the 2026-06-25 audit (commit `a16f35f`).

Method: four parallel reviewers, each with non-overlapping scope (Rust code; docs and ADRs; build/deps/CI/security; public API and repo hygiene). Findings deduplicated and cross-corroborated below.

## Verdict

**Ship with fixes.** Land the four critical items, the public-surface decisions (5, 6, 8), and the two ADR fixes (11, 12) and the project is ready to publish. Estimated work: one afternoon. The core library is in better shape than most first public Rust releases. The remaining issues are bounded edits.

## What landed since the 2026-06-25 audit

| Prior finding | Status | Evidence |
| --- | --- | --- |
| Autosync mutex-poison panic | Landed | `src/autosync.rs:356-361` `lock_inner` recovers and warns; test at `:482-512` |
| No graceful shutdown in production `main.rs` | Landed | `src/main.rs:98-99` `with_graceful_shutdown(shutdown::signal())`; `src/shutdown.rs` with SIGTERM test at `:42-58` |
| Silent env parse fallbacks | Landed | `src/config.rs:180-198` returns `InvalidEnv`; tests at `:458-481` |
| SSE first-connect dedup + ack stamping | Landed | `src/web.rs:217-232`, `src/autosync.rs:142-182`, `tests/sse.rs:337-371`, `tests/sse_demo_snapshot_only.rs` |
| `scenarios.rs` in public API | Open | `src/lib.rs:10` still `pub mod scenarios;`, 1217 LOC |
| Serialized cross-root scans | Open | `src/raw_view.rs:73-76` still sequential; acceptable today, callout retained |
| No `spawn_blocking` around render | Open (deliberate) | `web::*` runs render on the runtime thread; `benches/render.rs` pins per-folder cost per ADR-0022 |
| Loud env parse failures | Landed (per row 3 above) | |

## Critical

1. **Demo session mutex panic-loop hazard.** `src/demo/handlers.rs:163, 193, 237, 280` and `src/demo/state.rs:52` all call `state.sessions.lock().expect("session lock")` on a `std::sync::Mutex<SessionStore>`. The same fix already applied to `autosync` (`lock_inner` with `unwrap_or_else(|p| p.into_inner())`) was never extended to the demo. A single panic inside any session critical section poisons the mutex and every subsequent request panics. The demo is the only public-facing deployment.
   Fix: add `fn lock_sessions(&self) -> MutexGuard<'_, SessionStore>` mirroring `raw_view::lock_index`, route all five callers through it.

2. **Demo `derive_view` is unbounded `O(marks × folders)` per request.** `src/demo/handlers.rs:135-147` clones `base_raw` and replays every mark via `apply_mark_raw` on every request. No mark cap. A scripted `/mark` loop is a one-line DoS on the public demo container.
   Fix: cap marks per session in `SessionStore::append_mark` (e.g. 200, mirroring the existing `AtCapacity`-shaped soft refusal), or cache a derived `RawView` per session and invalidate on mark/reset.

3. **`cargo install missing-ebooks` plants a binary named `demo` in `~/.cargo/bin/`.** Confirmed: `cargo install --path . --root /tmp/mb-install --locked` produces both `missing-ebooks` and `demo` (3.5 MB). `demo` is a generic name that will collide with another tool on someone's PATH.
   Fix: rename in `Cargo.toml` via an explicit `[[bin]] name = "missing-ebooks-demo" path = "src/bin/demo.rs"` block. One-line change.

4. **`--help` is broken on both shipping binaries.** `missing-ebooks --help` falls through to `Config::load` and exits 2 on "no library roots configured" (`src/main.rs:18-22`). `demo --help` is silently ignored and starts the server (`src/bin/demo.rs:49`). A first-time `cargo install` user has no in-process discovery path for `--print-config` or the env vars. `examples/explore.rs:24` already has the right shape to copy.
   Fix: add a minimal `--help` block to both binaries, listing the env vars and pointing at `--print-config`.

## Important

5. **The whole `src/lib.rs` public surface is accidental.** 16 `pub mod`s exporting 58 public items. No third-party consumer exists. Every `pub` is there so `bin/demo`, `examples/`, `benches/`, or `tests/` can reach in. Either drop `lib.rs` and become binary-only, or lock the surface to a tiny intentional contract and document it.
   Fix: demote to `pub(crate)` everything except `Config`, `SearchLink`, `ConfigError`, `CONFIG_TEMPLATE`, and the narrow scanner surface examples actually use (`ScanSettings`, `RootScan`, `scan_warm`). Or move `lib.rs` modules under `main.rs` and ship binary-only. Recommend the latter for solo-maintenance simplicity.

6. **`scenarios.rs` and `synthetic.rs` ship in the public API.** `src/lib.rs:10, 13` exposes 1360 LOC of test-fixture types as `pub mod`. Production code (`scanner.rs:615`, `state.rs:498+`, `web.rs:332`, `web/render.rs:1650`, `demo/handlers.rs:327`) depends on `scenarios::touch` as a side-channel test-touch helper. Library consumers compile and link a synthetic audiobook seeder. Flagged in the prior audit; still open.
   Fix: gate behind a `fixtures` feature, `#[cfg(any(test, feature = "fixtures"))] pub mod scenarios;`. Enable in `[dev-dependencies]`, the demo bin via `required-features = ["fixtures"]`, and `examples/explore.rs`. Move the production `touch` helper into `scanner.rs` or `tests/common/mod.rs`.

7. **No body-size or request-timeout layer on the axum router.** `src/web.rs:55-57`. Inside the loopback threat model this is acceptable, but `config.rs:209-210` plus `main.rs:51-54` permit a non-loopback bind with only a warning. The warning model promises defense in depth and the router does not deliver it.
   Fix: add `DefaultBodyLimit::max(64 * 1024)` and `tower_http::timeout::TimeoutLayer::new(Duration::from_secs(30))` at the router. Cheap, brings the "we warn but do not refuse" promise honest.

8. **Six ADRs cite symbols that no longer exist.** Prior refactors (ADRs 0027, 0028, 0029) deleted `src/service.rs` and the `Cache` type. The earlier ADRs were never amended:
   - `docs/adr/0002-marker-writes-edit-cache-in-place.md:3, 11` cites `service::apply_mark_raw`, `service::mark`, `service::unmark`, `Cache::apply_marker_or_build`, `Cache::rebuild_root`.
   - `docs/adr/0009-htmx-swaps-one-section.md:5` cites `service::mark`.
   - `docs/adr/0025-library-coverage-derived-from-per-section-data-attrs.md:15` cites `service::render_section_from_raw`.
   - `docs/adr/0028:3` and `docs/adr/0029:3` cite `.scratch/architecture-review/findings.md` (the file does not exist).
   A first-time visitor who clicks into `docs/adr/` to learn how the project thinks gets a fictional map of the codebase.
   Fix: amendment headers on 0002, 0009, 0025 pointing at 0027/0028; inline or remove the missing-file citation in 0028, 0029.

9. **`.scratch/` is tracked, partially stale, and contradicts its own documented convention.** `docs/agents/issue-tracker.md:7-9` says the layout is `.scratch/<feature>/PRD.md` and `.scratch/<feature>/issues/<NN>-<slug>.md`. The seven tracked files use `plan.md` / `spec.md` with no `issues/` subdirectory. Three of those files cite paths that do not exist (`deep-dive/missing-ebooks-audit-2026-06-25.md`, `.scratch/architecture-review/findings.md`). ~3,240 lines of internal planning prose ships into a public repo.
   Fix: either curate `.scratch/` to match the documented convention and prune stale plans (candidate-1, candidate-4 have landed per ADRs 0027/0028/0029), or `.gitignore` the directory and keep working notes off public history. Recommend the latter for a solo workflow.

10. **`scenes/` agent dotfiles leak into the public artifact.** `src/web/page.rs:1-4` and `src/web/assets.rs:144` point doc-comments at `docs/superpowers/...`, which is gitignored (`.gitignore:18`). `cargo doc` renders these as dead links. `.claude/` is on disk and not in `.gitignore` (suppressed only by the author's global `~/.config/git/ignore`); a stray `git add .` on another machine commits it. `src/main.rs:1-2` calls the web UI "read-only" when the marker write endpoint is one of its defining features.
   Fix: remove or rewrite the `docs/superpowers/` doc-comments; add `.claude/` to `.gitignore`; reword `main.rs:1-2`.

11. **`compute_pushes` holds the autosync lock across render of every subscribed (root, mode) pair.** `src/autosync.rs:405-421` packages and renders under the same lock that `subscribe_inner` needs to register a new SSE sender (`:281-311`). On a busy 50-root library, a new `/events` attach can wait through 100 packagings + renders. The bench numbers folded in at commit `a6a4c42` show the 50k case in milliseconds. The plan at `.scratch/autosync-section-cache/` already addresses this; it should land before claiming 50k-folder readiness.
   Fix: either hoist `package_section` + `single_oob_section` out of the lock (only the diff and the seed-update need it) or land the queued render cache.

12. **`attach` always renders the snapshot even when it will be discarded.** `src/autosync.rs:160-172` packages every root and renders OOB HTML even on first connect when `send_snapshot` is false. With a 10s autosync and reconnect churn, this is per-connect waste, not per-tab.
   Fix: split into `seed_hashes_only(raw, mode) -> Vec<u64>` and `snapshot_payload(raw, mode, links) -> String`; call the second only on the snapshot branch.

13. **Per-entry directory scan errors are silently dropped.** `src/scanner.rs:509-513` uses `read_dir(..).flatten()` and `entry.file_type()?-else` so a transient SMB hiccup, antivirus lock, or permission glitch on one entry makes that audiobook vanish from the scan with no log. Silent data loss in a tool whose entire purpose is "tell me what is missing."
   Fix: replace `entries.flatten()` with explicit `match`; `tracing::trace!(dir, %err, "skipping unreadable entry")` on the error arm.

14. **Missing crates.io / publish hygiene.** `Cargo.toml` has no `keywords`, `categories`, `homepage`, no `include` / `exclude`, and `publish` defaults to true. An accidental `cargo publish` today would ship `.scratch/` (200 KB), `benchmarks/` (196 KB), the full curated audiobook fixture tree, and all of `docs/`. Set `publish = false` or commit to crates.io with an exclude list.
   Fix: pick one. If binary-only via Docker + `cargo install --git`, set `publish = false`. If crates.io, add `keywords`, `categories`, and an `exclude` list covering `.scratch/`, `benchmarks/`, `tests/fixtures/`, `demo/`, `docs/`, `.github/`, `.githooks/`.

15. **No supply-chain integrity check on vendored JS.** `assets/htmx.min.js` is htmx 2.0.4 vendored with no in-file version stamp and no checksum check. The version lives only in `docs/adr/0009-htmx-swaps-one-section.md:11`. `assets/htmx-sse.js` has the same problem. The artifact's provenance is invisible.
   Fix: add a header comment to each vendored file naming the upstream URL, version, tag/commit, SPDX license. Add a small test (`tests/assets/integrity.rs` or shell) that compares `sha256(htmx.min.js)` against a pinned digest.

16. **`examples/scan_bench.rs` (1578 LOC) and `examples/tree_bench.rs` (320 LOC) are benchmarks, not examples.** They dwarf the one real example (`explore.rs`, 381 LOC) and confuse a first-time visitor browsing `examples/`. Both have docstrings explicitly calling themselves benchmarks.
   Fix: move to `benches/` (with `harness = false` if not Criterion) or rename and document why they live in `examples/`.

17. **README has no build-from-source preface, no license note, no link to repo or releases.** `README.md:36-44` jumps to `cargo run --release` with no mention of cloning or installing Rust. The AGPL-3.0 choice that matters most to self-hosting users is never named in the README. The `cargo run --example explore -- mixed-forest` path that solves "I have no library to point at" is buried at section 7 when it should be promoted under "Live demo" as a one-line local-try alternative.
   Fix: one-paragraph "Build from source" preface; one-line License section; promote the explore harness.

18. **`benchmarks/README.md` and `tests/fixtures/example-nas/README.md` expose personal infrastructure names.** `jane-core`, `jane-2`, `jane-nas`, `/mnt/jane-nas/...`, references to a private `server-configs` repo. Not a secret leak; the example-nas readme already redacted partly. The published numbers are tied to one person's homelab.
   Fix: role-based names ("storage host", "SMB client"). Or extract the experiment log to `benchmarks/EXPERIMENTS-2026-06.md` and keep the methodology clean.

19. **`multiple-versions = "warn"` in `deny.toml` is effectively a no-op.** `deny.toml:28` warns, but cargo-deny defaults to the host target only. The real duplicates live behind `wasi`/`wasip2`/`wasip3` cfg. The policy reads as "we ban duplicates" but bans nothing.
   Fix: add `[graph] all-features = true` and explicit `targets = [{triple = "x86_64-unknown-linux-musl"}, {triple = "aarch64-unknown-linux-musl"}]`.

20. **No Dependabot or Renovate.** Weekly `audit.yml` cron catches CVEs in transitive deps; nothing nudges routine version bumps. For a solo maintainer this is the difference between drift compounding silently and a small PR per dep per week.
   Fix: minimal `.github/dependabot.yml` for cargo + github-actions ecosystems, weekly schedule.

21. **No SBOM / vulnerability scan in the Docker publish pipeline.** `docker-publish.yml` builds and pushes; no `--provenance` / `--sbom` flags, no `actions/attest-build-provenance`. Lowest-effort high-leverage addition for a first ghcr.io release.
   Fix: enable buildx provenance and SBOM; or add a Trivy / Grype step.

22. **`getrandom 0.2` is a direct dep with one call site.** `Cargo.toml:37` pins 0.2 because `src/demo/handlers.rs:84` calls `getrandom::getrandom(&mut buf)`. Migrating to the 0.3 `fill` API (or using `rand` which is already in the dev tree) drops the direct dep entirely and removes an "abandoned major" footgun.

## Minor

23. `src/autosync.rs:900-901` carries two em dashes in a code comment. Only em dashes anywhere in the repo and a violation of the AGENTS-level rule.
24. `src/web.rs::ascii_escape` (`:262-275`) allocates a `format!` per non-ASCII char; `write!` saves the heap allocation.
25. `SessionId(pub String)` (`src/demo/session.rs:12`) exposes the field; touched from one production site. Make private; add `SessionId::new`.
26. `autosync::section_content_hash` uses 64-bit `DefaultHasher` (`:100-104`). Collision suppresses one tick's diff silently. Doc the recovery on next changed tick, or move to xxhash 64×2.
27. `apply_mark_raw` (`raw_view.rs:26-55`) silently swallows out-of-range roots and `Failed` sections. At least `tracing::warn!` so a "the mark did not stick" user report leaves a breadcrumb.
28. `RawViewStore`, `RawView`, `Applied`, `WriteError`, `WriteFailure` are `pub` (`state.rs:47, 197, 208, 228`) but only `pub(crate)`-reachable. Demote.
29. `AtomicU64` lives inside `Mutex<AutosyncInner>` and is bumped under the lock (`autosync.rs:212, 417`). Either move out or make it a `u64`.
30. `web::events` (`web.rs:243-256`) synchronously builds the snapshot inside `attach`; on a cold cache the SSE handshake blocks for tens of seconds.
31. ADR date and section-header formatting drifts: 0001-0022 have no date headers, 0023-0024-0030 do, 0025-0029 do not. Section headers (`## Context / ## Decision / ## Consequences`) appear in 0023-0025-0030 and not in the rest. Pick a rule.
32. `Dockerfile` `FROM rust:1.96-alpine` and `FROM alpine:3.21` float on minor; digest-pin for reproducibility.
33. `assets/htmx-sse.js:9` carries a `/** @type {import("../htmx").HtmxInternalApi} */` path that points outside `assets/` to a file that does not exist here. Upstream remnant; strip it.
34. `.github/` has no `SECURITY.md`. One file pointing at the loopback threat model and how to report a real issue is cheap and signals seriousness.
35. `tsconfig.json` + `types/htmx.d.ts` at repo root is surprising for a Rust crate. One forward reference to ADR-0016 in the README's Development section avoids the "wrong repo?" reaction.
36. `tests/fixtures/curated/` carries `._*` AppleDouble files. Probably intentional fixture data for the scanner; document it in `tests/fixtures/curated/README.md` so someone does not "clean" them.
37. `docs/agents/triage-labels.md:15` carries an instruction ("Edit the right-hand column to match...") that belongs in a template, not a committed project doc.
38. `docs/agents/domain.md` is mostly generic skill-family documentation; the two repo-specific facts could fold into `CLAUDE.md`.
39. `LICENSE` has no project-specific copyright header. Conventional and fine; one line naming the holder and year closes a small gap.
40. `web/render.rs` is 1884 LOC; the test module at the bottom (1300+ LOC) is hiding signal. Split into `web/render/tests/{section.rs, gap_summary.rs, oob.rs, packaging.rs}`.

## Complexity that does not earn its keep

- **`src/scenarios.rs` in the public lib** (1217 LOC, 50 KB). See issue 6.
- **`src/synthetic.rs` in the public lib** (143 LOC). See issue 6, same fix.
- **`autosync::render_oob_section`** (`autosync.rs:79-87`) is `#[cfg(test)]`-only, kept to assert a byte-equality contract between two prod call sites. Replace with `debug_assert!` in `compute_pushes` and delete; the test is testing its own definition.
- **`RawViewStore` vs `AppState` split.** `AppState` is a one-field wrapper around `RawViewStore` plus `Autosync`. The split is justified by ADR-0027 (handlers reach for `state.config` directly). Inlining `RawViewStore`'s fields would not carry test surface costs. Not urgent; flagging.
- **`Applied { raw, created }` and `WriteFailure::Failed { raw }`** both thread the raw view alongside their semantics. A `struct WriteOutcome { raw: Arc<RawView>, result: Result<bool, WriteError> }` lets the caller match and drops one enum arm.
- **`src/lib.rs` as a public library** at all. See issue 5. The most impactful complexity reduction available: become a binary-only crate and the entire question of `pub` vs `pub(crate)` disappears.

## Performance findings

- `package_view` is uncovered by `benches/render.rs`; `compute_pushes` calls it per-tick per-root. At 50k folders `tree::build_forest` (heap-allocating per call) is the dominant cost above ~10k folders. Speculation, not measured. The queued section-content render cache addresses this.
- `RawViewStore::write_mark`'s `Arc::make_mut` (`state.rs:287`) deep-clones the entire `RawView` when any subscriber still holds an `Arc<RawView>` from `current()`. Long-lived SSE channels release their `Arc` after `attach` returns, so this should be rare; the failure mode is silent (writes double in cost). Add a `tracing::debug!("write_mark cloned the raw view: outstanding readers = N")` so it is detectable in prod.
- `examples/scan_bench.rs` measures real-config SMB cold runs; `benches/render.rs` is synthetic. The arch-review numbers are synthetic. The two are not directly comparable.
- Asset pipeline is deliberately empty (no minification, no fingerprint hash) and the right call for a single-user loopback app. The 48.9 KB CSS and 58.3 KB JS are hand-rolled, source-readable, and re-fetched once.

## Test gaps

- No test for `write_mark` on a `RootScan::Failed` section. Behavior is "marker hits disk; in-memory view does not flip until next rescan." Pin this.
- No proptest on `tree::build_forest` (`src/tree.rs:163-204`). Natural-sort and placeholder-overwrite invariants are good proptest material.
- No test that the autosync loop survives a panic inside `package_section` or `single_oob_section`. `lock_inner` recovers a poisoned mutex; the spawned loop task itself crashes silently if render panics inside the lock.
- `tests/sse.rs` does not cover: SSE attach during a `write_mark` (does the new subscriber see the post-mark view?), simultaneous `/rescan` and `/events`, or a `WriteFailure::Failed` taking the inline-alert path with an active SSE subscriber.
- `tests/curated_contract.rs` has no rendered-HTML golden file. A 1500-LOC Maud renderer will drift without one.
- `apply_mark_raw` has no test for a root-mark (`rel == "."`) flipping a deep descendant.
- `scanner.rs:509` per-entry-error path is untested. A `chmod 000` file or a dangling symlink in a unix test would catch it.
- `web::events` cold-cache response latency is uncovered; the synchronous snapshot build is invisible to the test suite.

## Strengths

- Toolchain pinned through every layer: `rust-toolchain.toml`, `Cargo.toml:5`, `Dockerfile:4`. No drift surface.
- Release profile is serious: `lto = "fat"`, `codegen-units = 1`, `strip = true`. 3.7 MiB binary; ~12-13 MiB compressed image per arch.
- CI is lean and parallel: 7 jobs, all independent, `Swatinem/rust-cache@v2` where it pays. Pre-commit hook scopes by staged-file path so unrelated commits stay fast.
- Multi-arch publish on native runners (no QEMU), digest-merge, smoke-gated.
- Dep tree is conservative and modern (axum 0.8, tokio 1.52, maud 0.27, rayon 1.10). 177 crates, no abandoned, all licenses on the allow-list, weekly RustSec cron.
- Test coverage is heavy and well-aimed. `~6k` test LOC; the `rebuild_count` test seam (`state.rs:54`, `autosync.rs:212`) is the right shape (counter is free in prod, diffed in tests).
- Render byte-equality is pinned in tests (`web/render.rs:649`, `autosync.rs:895`); the htmx-first-colon-split test is a real prior-regression fence.
- Loopback threat model honored end-to-end: `Config::default()` binds 127.0.0.1, `main.rs:51-54` warns on non-loopback, `docker-compose.yml:10` ships `127.0.0.1:13379:13379`. No auth code path to forget.
- Container drops root via `su-exec` with PUID/PGID overridable, tested by a shell harness without Docker.
- Style discipline across docs: no em dashes (except the two in `autosync.rs:900-901`), no AI tells, no superlatives, unwrapped paragraph lines.
- The ADR practice is real: 30 ADRs with internal cross-links and a supersession chain (0022 → 0027 → 0028 → 0029, 0023 → 0024 → 0030).
- Public demo at `demo-missing-ebooks.noahbaculi.com` plus `cargo run --example explore -- mixed-forest` gives a curious visitor two zero-friction paths to try the product.

## Release punch list (ordered)

1. Rename `demo` binary to `missing-ebooks-demo` (issue 3).
2. Add `--help` to both binaries (issue 4).
3. Fix demo session mutex poison hazard (issue 1).
4. Cap demo session marks (issue 2).
5. Move `scenarios.rs` + `synthetic.rs` behind a `fixtures` feature (issue 6).
6. Decide library-or-binary: either demote everything to `pub(crate)` or drop `lib.rs` (issue 5).
7. Set `publish = false` or add `Cargo.toml` `exclude` + `keywords` + `categories` (issue 14).
8. Add `.claude/` to `.gitignore`; rewrite or remove the `docs/superpowers/` doc-comments; reword `main.rs:1-2` (issue 10).
9. Add amendment headers to ADRs 0002, 0009, 0025; fix `.scratch/architecture-review/findings.md` citations in 0028, 0029 (issue 8).
10. Resolve `.scratch/`: either curate to match its documented convention or `.gitignore` it (issue 9).
11. Add `DefaultBodyLimit` and `TimeoutLayer` to the router (issue 7).
12. Add scanner per-entry error log (issue 13).
13. Stamp vendored JS with version/license headers + a checksum test (issue 15).
14. Move `scan_bench.rs` / `tree_bench.rs` out of `examples/` (issue 16).
15. README: build-from-source preface, license line, promote `explore` (issue 17).

The autosync render-cache plan (issue 11), the snapshot-only-on-snapshot-branch split (issue 12), Dependabot (20), SBOM (21), and the `getrandom 0.2` drop (22) can land post-0.1.0 without harm.
