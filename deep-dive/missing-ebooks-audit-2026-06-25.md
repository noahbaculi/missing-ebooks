# missing-ebooks — Architectural Audit

Scope: full Rust crate (~10.8k LOC, ~6k of which are tests). Senior-level compact audit.

### Architecture Summary

- One axum app over a single shared `AppState`. `RawViewStore` (`src/state.rs`) holds an `Option<CacheEntry>` behind a `tokio::Mutex` (the "raw view store" slot, TTL-bounded) plus an `Arc<std::sync::Mutex<DirIndex>>` (process-lifetime mtime map). Both views (gaps-only and show-all) render at request time from the same raw scan.
- Scanner (`src/scanner.rs`) is a level-synchronous BFS, parallelized within a level via rayon over a global pool sized from `config.scan_concurrency`. Symlinks are not followed by construction. The whole walk runs inside a single `spawn_blocking`.
- Push channel: `autosync.rs` is a lazy SSE fan-out that spawns on first subscriber, calls `store.refresh()` on a timer, diffs per-section rendered HTML by `DefaultHasher`, and pushes only changed sections. Loop dies when the last subscriber leaves or `Weak<AppState>` fails to upgrade.
- `demo/` is a separate `bin/demo.rs` entry point that wraps the same renderer but never writes to disk; visitor marks live in an in-memory `SessionStore` keyed by 128-bit random cookies.

### Key Decisions

- **Single-flight is the `tokio::Mutex` itself, not a `Notify` or watch-channel.** `current`, `refresh`, `rescan`, and the cold branches of `write_mark`/`remove_mark` hold the cache lock across `build_view().await`. Cheap to reason about; means one slow root scan blocks every other store op including marker writes. Watch-channel readers would decouple but were not chosen — ADR-0022/0027 commit to render-at-request-time.
- **Marker write is `O_EXCL` create of an empty file (`state.rs:396-400`), not write-then-rename.** Atomic by virtue of zero content; no fsync. Reasonable for a sentinel, but means a crashed concurrent unmark could leave the slot stale until the next mtime tick.
- **Path-traversal defense is double-`canonicalize` + `starts_with`** (`state.rs:381-413, 420-453`). Covers both `..` and intra-library symlink escapes. TOCTOU window exists but is out of the single-user threat model.
- **Render is synchronous on the request task.** Maud renders to one big `String` with no streaming; `count_gaps` recurses every render. ADR-0022 bets per-folder cost stays bounded — not stress-tested adversarially.
- **`autosync` baseline-hash seeding deliberately does not overwrite** an existing baseline on new-subscriber attach (`autosync.rs:210-251`); the cost is one redundant section event for a late subscriber, the benefit is no pending-diff erasure for earlier tabs.

### Flags

- **`autosync.rs` panics on every lock acquire** (`expect("autosync mutex poisoned")` at `:245,274,283,295,332,344,353`). Inconsistent with `raw_view.rs:119-123` which recovers from `DirIndex` poisoning. A panic inside `compute_pushes` poisons the registry; the respawn-on-subscribe path then panic-loops. Low likelihood, high blast radius. Recover via `PoisonError::into_inner` like the index does.
- **Env numeric overrides silently fall back to default on parse error** (`config.rs:174-186`, all `.parse().ok()`). `MISSING_EBOOKS_PORT=garbage` is indistinguishable from unset. Should warn or hard-fail.
- **Production `main.rs` has no `with_graceful_shutdown`.** `axum::serve` is fired and forgotten; Ctrl-C drops in-flight requests. The demo binary gets this right (`bin/demo.rs:87-89`), the production one does not.
- **Cross-root scanning is serialized through `Arc<Mutex<DirIndex>>`** held across `scanner::scan_root` inside `spawn_blocking` (`raw_view.rs:93-95`). Within-level rayon parallelism is real; root-level parallelism is left on the table. Matters most on multi-root NAS setups, exactly the deployment target.
- **No `spawn_blocking` around `render_view`.** A 50k-folder render pins a runtime worker. Same applies to `compute_pushes`, which re-renders every subscribed `(mode, root)` every tick before the diff hash decides whether to push — render cost is paid even when nothing changed.
- **`scenarios.rs` (1217 LOC of seeders) is shipped in `lib.rs`** as `pub mod scenarios`. Used by `bin/demo` at runtime, also reachable by any library consumer. Either gate behind `#[cfg(any(test, feature = "scenarios"))]` or accept the surface.
- **Rayon `build_global` failure is downgraded to a tracing warn** (`main.rs:64-69`). Survives today because it's only called once, but the failure mode is silent if that ever changes.
- **`scanner.rs:454, 564`**: `dir.strip_prefix(root).ok()` drops folders silently if the prefix doesn't match. Defensive but quiet; consider `expect` or a debug-only assert.

### Edge Cases & Failure Modes

- **Cold-scan stalls block marker writes.** A 30-second NAS walk inside `current`/`refresh` holds the cache mutex; any `/mark` POST queues behind it. Operator-visible as button latency immediately after startup or `/rescan`.
- **`CachedDir.mtime` is captured before `read_dir`** (`scanner.rs:441, 572`). If a directory is modified mid-listing, the cached mtime is stale and the next warm scan trusts the stale listing until something else invalidates. Marker self-writes call `invalidate_index` to cover the common case; external writes (rsync, Sonarr) ride on the next mtime change.
- **Mutex poisoning of `DirIndex`** is swallowed (`raw_view.rs:119-123`). A panic mid-walk leaves whatever entries were inserted; the comment claims self-heal via mtime, but a half-populated row whose `CachedDir` was never written stays absent (re-listed next pass) so the comment holds.
- **SSE subscriber mpsc is bounded at 16** (`autosync.rs:122`); slow clients are dropped on `try_send` failure. Lossy by design. Dropped subscribers see the stream end when their receiver next observes channel-closed.
- **Demo session table is unbounded only by config max** (`demo/handlers.rs:124,149-151`). 503 at cap is honored, but a flood of unique visitors at the threshold thrashes the reaper.
- **Production has no auth, no CSRF, no rate limit, no request size limit beyond axum defaults.** Bind defaults to 127.0.0.1; non-loopback binds emit a warning but no refusal. A LAN attacker on a non-loopback bind can forge `/mark` writes.
- **`rayon::ThreadPoolBuilder::build_global` is one-shot.** Not exercised today; would fail silently if anyone refactors `Config::load` to run twice.
- **Non-UTF-8 filenames** are `to_string_lossy()`'d through the classifier (`scanner.rs:114`); audio/ebook tagging requires UTF-8 extension match, so weirdly-encoded extensions are treated as neither.

### Testability

- **Test coverage is heavy and well-aimed.** ~6k test LOC in-tree covers single-flight, cache TTL, warm-vs-cold parity, parallel determinism (`scanner.rs:657`), symlink non-following, escape rejection, OOB byte-equality between snapshot and per-tick push, ETag and `If-None-Match` weak-prefix, HX-Trigger non-ASCII escape, baseline-hash seeding without overwrite, abort-and-respawn lifecycle, env-override layering, template round-trip. Integration tests in `tests/sse.rs` and `tests/curated_contract.rs` drive the production router via `tower::ServiceExt::oneshot`.
- **Property tests are confined to `query.rs:102-133`** (idempotence, emptiness, no edge separators after cleaning). The scanner classifier, exclude-glob matcher, and HX-Trigger ASCII escape are all good candidates and currently example-based only.
- **Hard-to-test surfaces:**
  - **Render perf under adversarial trees** has no benchmark in `benches/`. Manual benches in `examples/scan_bench.rs` and `examples/tree_bench.rs` exist, but no `criterion` regression guard. Add one if the 50k-folder claim matters.
  - **Autosync timing** is tested via injected interval and `abort_loop_for_test`, but the lock-poisoning recovery path is untestable because `expect` panics. Switching to `into_inner` would also make this testable.
  - **`build_global` failure** can't be exercised in unit tests since `ThreadPoolBuilder` writes a process-global. Tests use ad-hoc pools (`scanner.rs:649-654`); the production wiring path is uncovered.
  - **Concurrent scan + mark interleavings** are tested at the API surface but the lock-held-across-await design makes interleavings observable only as queueing, not as races. Loom or shuttle could exercise the std-mutex paths.
  - **`scenarios.rs` in the public API** means tests against the library are effectively coupled to a test fixture module. Acceptable today; will hurt if the crate ever ships as a library dependency.

---

## Prioritization and Action Plan

The findings above are signal-dense but unranked. This section is the reviewer's pass: what to grab first, what to defer, and where the audit itself was weak.

### Fix now (small, clear wins)

1. **Autosync lock poisoning recovery.** Replace every `.lock().expect("autosync mutex poisoned")` in `autosync.rs` with the `into_inner`-on-poison pattern already used in `raw_view.rs:119-123`. One small helper, ~30 lines of diff, removes a panic-loop failure mode entirely.

2. **Graceful shutdown in production `main.rs`.** Lift `with_graceful_shutdown(ctrl_c)` out of `bin/demo.rs:87-89` into a shared helper and call it from both binaries. No behavior change for the demo, real benefit for production.

3. **Loud env parse failures in `config.rs:174-186`.** Either warn-and-fall-back or hard-fail. `.parse().ok()` is the wrong default for an operator-facing knob.

4. **Gate `scenarios.rs` behind a feature.** `#[cfg(any(test, feature = "scenarios"))]` on the `pub mod scenarios;` in `lib.rs`, then add `features = ["scenarios"]` on the demo binary's dev-dep entry. Shrinks the public surface and the release binary by ~1200 LOC of seeder code that production never executes.

### Worth doing (medium effort)

5. **Memoize per-section renders in `autosync::compute_pushes`.** Today every subscribed `(mode, root)` re-renders on every tick before the hash decides whether to push. Cache the last `(raw_view_version, mode, root) -> (html, hash)` tuple in the registry; only re-render when the underlying `RawView` Arc identity changes. Bigger impact than moving render to `spawn_blocking`.

6. **Replace silent `strip_prefix(root).ok()` at `scanner.rs:454,564`** with `debug_assert!` plus an `expect` in release. If the prefix invariant ever breaks, fail loudly in test and skip-with-warn in production.

7. **One criterion bench for `render_view` against a synthetic 50k-folder tree.** Validates the ADR-0022 per-folder cost claim and gives a regression guard. The existing `examples/tree_bench.rs` already has the seeder shape; lift it into `benches/`.

### Strategic (real refactor, only if pain shows up)

8. **Decouple readers from the scan via `tokio::sync::watch<Arc<RawView>>`.** Right now the cache `tokio::Mutex` is held across the whole scan await, so marker writes queue behind a cold scan. A watch channel lets every reader (`/`, `/events` snapshot, `/mark`'s in-place edit path) see the latest finished `Arc<RawView>` without contending with the writer. The writer single-flights via `tokio::sync::Notify` or a dedicated task. Biggest correctness-shaped win in the audit and the most invasive. Don't do it preemptively. Do it the first time a user reports "I clicked mark and the button hung for 20 seconds."

9. **Shard the `DirIndex` per root.** Replace `Arc<Mutex<DirIndex>>` with `Arc<HashMap<RootId, Mutex<DirIndex>>>` (or a `DashMap`). Lets `raw_view.rs:74-76` actually scan roots in parallel instead of serializing them through the global mutex. Real win for multi-root NAS, the stated deployment target. Lower risk than (8); do this one first if either becomes pressing.

### Defer or accept

- **No auth / CSRF / rate limit.** Threat model is loopback; the warning on non-loopback bind is enough. Half-measures are worse than nothing.
- **Marker writes are `O_EXCL` create instead of write-then-rename.** Zero-byte files don't have torn writes.
- **`rayon::build_global` one-shot.** Called once. A `debug_assert` that it hasn't been called before would document the invariant without adding behavior.
- **TOCTOU on canonicalize-then-open.** Single-user tool, not exploitable.

### Where the audit itself was weak

- **It overstated the render risk.** The "50k-folder libraries pin a worker for 100+ ms" claim was made without measuring. The criterion bench in (7) is what gives that claim weight; without it, the recommendation is speculation.
- **It double-counted `scenarios.rs`.** The "shipped in `lib.rs`" flag and the "not production code" framing in the survey contradict each other. The feature-gate in (4) is the resolution.
- **It didn't categorize.** A reader of the audit didn't know which finding to grab first. This section is what was missing.
