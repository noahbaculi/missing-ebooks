# Release blockers (audit critical items)

From the 2026-06-26 pre-release audit (`deep-dive/missing-ebooks-audit-2026-06-26.md`, "Critical" 1-4). Four items that together unblock the first public release tag. All sit on the demo binary or on the binaries' CLI surface. The production server logic (`src/state.rs`, `src/web/`, `src/scanner.rs`, `src/autosync.rs`, `src/raw_view.rs`) is unchanged; the only production-side edits are `Cargo.toml` (binary rename, feature placeholder, clap dep) and `src/main.rs` (clap-derived `Cli` struct wrapping the existing startup).

The release-blockers spec is the first piece of the post-audit work. Specs B (public surface lockdown), C (docs cleanup), D (router hardening), E (vendor / supply chain), and G (examples reorg) follow under their own feature dirs. F (autosync section cache) already has a spec. H bundles the minors; I and J defer post-release.

## Why

The audit ranked these four as Critical:

1. **Demo session mutex poisons on any thread panic.** The fix the project already shipped for `autosync` (`src/autosync.rs:356-361`) and `raw_view` (`src/raw_view.rs:119-123`) was never extended to the demo. Five call sites in `src/demo/handlers.rs` and `src/demo/state.rs` `.expect("session lock")` on a `std::sync::Mutex<SessionStore>`. A single panic under any of those guards puts the demo into a panic-loop until the container restarts. The demo is the only public-facing deployment.
2. **Demo `derive_view` is unbounded `O(marks × folders)` per request.** Per `src/demo/handlers.rs:135-147`, each request clones the entire `base_raw` then replays every session mark via `apply_mark_raw` (`src/raw_view.rs:45-54`). A scripted `POST /mark` loop grows `M` unboundedly within one session, and per-request CPU grows linearly with `M`. On a `big-library` scenario (50k folders) `M=1000` is 50M ops per request, synchronous on a runtime worker. One scripted client can stall the demo for every other visitor.
3. **`cargo install missing-ebooks` plants a binary called `demo`** in `~/.cargo/bin/`. The name is generic and will collide with an unrelated tool on someone's PATH. Confirmed in the audit by `cargo install --path . --root /tmp/mb-install --locked`.
4. **`--help` is broken on both binaries.** `missing-ebooks --help` falls through to `Config::load` and exits 2 on "no library roots configured" (`src/main.rs:18-22`). `missing-ebooks-demo --help` is silently ignored and the server starts (`src/bin/demo.rs:49`). A first-time `cargo install` user has no in-process discovery path for the env vars or for `--print-config`.

While restructuring the demo to fix #2, the audit's "demo `/unmark` is missing" UX inconsistency comes along for free: the production toast's Undo button POSTs to `/unmark` (`assets/app.js` + `src/web/page.rs:225-234`), but the demo router has no `/unmark` route, so clicking Undo in the demo silently 404s. The set-based mark storage makes adding the route a five-line change; we close the inconsistency in the same commit.

## End state

The production binary stays exactly as it is, including the in-place mutation of the cache by `apply_mark_raw` (ADR-0002). The demo binary changes shape:

- The bin is renamed `missing-ebooks-demo` and is gated behind a new `fixtures` Cargo feature, declared empty in this spec and consumed by spec B (which gates `scenarios.rs` and `synthetic.rs` under the same feature). `cargo install missing-ebooks` no longer plants a `demo` binary.
- Both binaries gain clap-derived CLI structs. `--help`, `--version`, `--print-config`, `--config` (production) and `--help`, `--version`, `--scenario`, `--bind` (demo) work. Env vars (`MISSING_EBOOKS_*`, `DEMO_*`) continue to be the primary configuration path; CLI flags layer on top per the existing precedence in `Config::load`.
- The demo's session storage moves from `Vec<Mark>` to `HashSet<(usize, String, Marker)>`. Repeated identical marks are no-ops at insert time. Per-session size is structurally bounded by the loaded scenario's `|markable folders × marker kinds|`, not by attacker behavior.
- `POST /mark` and `POST /unmark` validate `(root, rel)` against `base_raw` and return `400 unknown folder` for paths that don't exist (matching the existing 400 for out-of-range roots at `handlers.rs:187-189`). Garbage marks no longer reach the set.
- A new `MarkOverlay` borrow-on-the-session-set replaces the clone-and-replay path. `package_view_with_overlay(&base, &overlay, mode) -> FlaggedView` walks `base` once, consults the overlay per folder, and produces a `FlaggedView` whose internal folders carry overlay-corrected `missing_ebook` and `cover_files`. The production `render_view` consumes it unchanged. Per-request demo cost drops from `O((M+1) × F)` to `O(F × depth)` where depth is typically 2-3.
- `DemoState::lock_sessions` mirrors `raw_view::lock_index`'s poison-recovery pattern. All five `.expect("session lock")` call sites route through it.
- The demo router gains `POST /unmark`. With set storage, the handler body is one `marks.remove(&key)` call. The toast Undo button stops silently 404ing.

No per-session mark cap. The set is bounded structurally by the scenario. Aggregate worst-case memory at `DEMO_MAX_SESSIONS=1000` with every visitor on `big-library` marking every markable folder with every marker kind is ~90 MB; legitimate exploration is in the kilobytes per session.

## Data flow

### Mark / unmark / render before

```
POST /mark  →  session.marks.push(Mark { root, rel, kind })   // no validation of rel
GET /      →  derive_view(base, &session.marks, mode):
                clone(base)                                    // O(F) memory + CPU
                for each mark: apply_mark_raw                  // O(M × F)
                package_view                                   // O(F)
              render_view                                      // O(F)
                                  total per request: O((M+1) × F)
```

### Mark / unmark / render after

```
POST /mark  →  if !folder_exists_in_base(base, root, rel): 400
            →  session.marks.insert((root, rel, kind))         // O(1)
            →  render_with_overlay(base, &session.marks, mode)

POST /unmark →  if !folder_exists_in_base(base, root, rel): 400
            →  session.marks.remove(&(root, rel, kind))        // O(1)
            →  render_with_overlay(base, &session.marks, mode)

GET /      →  render_with_overlay(base, &session.marks, mode)

render_with_overlay(base, marks_set, mode):
  let overlay = MarkOverlay::new(marks_set);                   // borrows, no alloc
  let flagged = package_view_with_overlay(base, &overlay, mode);
                                  // walks base once: O(F)
                                  // per folder: O(depth) overlay probes via ancestors
                                  // total: O(F × depth)
  render_view(&flagged, ...)                                   // unchanged
```

## Storage

### `src/demo/session.rs`

```rust
pub type MarkKey = (usize, String, Marker);  // (root_index, rel_path, marker_kind)

struct Session {
    marks: HashSet<MarkKey>,
    last_seen: Instant,
}

impl SessionStore {
    /// Insert returns `true` if the mark was newly added, `false` if already present.
    /// Returns `Err(UnknownSession)` if `sid` is not in the store.
    pub fn insert_mark(&mut self, sid: &SessionId, key: MarkKey) -> Result<bool, UnknownSession>;

    /// Remove returns `true` if the mark was present and removed.
    pub fn remove_mark(&mut self, sid: &SessionId, key: &MarkKey) -> Result<bool, UnknownSession>;

    /// Borrow the mark set for overlay construction. Held under the session lock
    /// for the duration of the render.
    pub fn marks(&self, sid: &SessionId) -> Result<&HashSet<MarkKey>, UnknownSession>;

    pub fn clear_marks(&mut self, sid: &SessionId);   // unchanged semantics
}
```

The `Mark` name is dropped from stored state. The request struct in `src/web.rs::MarkRequest` (`pub root, rel, kind`) stays as the wire shape; the handler builds a `MarkKey` from it directly.

### Validation

```rust
// In src/demo/handlers.rs:
fn folder_exists_in_base(base: &RawView, root: usize, rel: &str) -> bool {
    let Some(scanner::RootScan::Walked { folders, .. }) = base.get(root) else {
        return false;
    };
    if rel == "." {
        return true;  // ADR-0005: every walked root is itself flaggable.
                      // Root folders carry empty rel_path in ScannedFolder, not "." ,
                      // so the equality branch below would miss it.
    }
    let target = PathBuf::from(rel);
    folders.iter().any(|f| f.rel_path == target)
}
```

`O(F)` per validation. Runs at most twice per user click (mark, later unmark). For the largest scenario this is sub-millisecond. No precomputed lookup table; if a follow-up needs it, build a `HashSet<(usize, PathBuf)>` at base load time.

## Overlay

### `src/demo/overlay.rs` (new)

```rust
use std::collections::HashSet;
use std::path::Path;
use crate::demo::session::MarkKey;
use crate::marker::Marker;

pub struct MarkOverlay<'a> {
    marks: &'a HashSet<MarkKey>,
}

impl<'a> MarkOverlay<'a> {
    pub fn new(marks: &'a HashSet<MarkKey>) -> Self { Self { marks } }

    /// For a folder at (root, rel), compute the overlay-corrected state.
    ///
    /// Walks the folder's ancestors (including itself). If any ancestor is in the
    /// marks set under the same root, `cleared_by_ancestor` is true. Markers exactly
    /// at this folder are listed in `exact_markers` so the caller can append the
    /// marker filenames to `cover_files`.
    ///
    /// Depth in audiobook libraries is typically 2-3, so this is O(depth)
    /// HashSet probes per folder.
    pub fn effective_state(&self, root: usize, rel: &Path) -> EffectiveState {
        let mut state = EffectiveState::default();

        // Build the rel string for the folder itself, then walk ancestors.
        // PathBuf::ancestors yields the path and every prefix down to "";
        // for our purposes we want all of them as ASCII rel strings.
        for ancestor in rel.ancestors() {
            let ancestor_rel = if ancestor.as_os_str().is_empty() {
                "."  // root folder convention (ADR-0005)
            } else {
                // ancestor is a borrow of rel up to a path boundary, so the
                // OsStr → str conversion is the same one the scanner does
                // (rel_path is always UTF-8 ASCII per the scanner's invariants).
                match ancestor.to_str() {
                    Some(s) => s,
                    None => continue,
                }
            };

            for kind in Marker::ALL {
                let key = (root, ancestor_rel.to_owned(), kind);  // see allocation note
                if self.marks.contains(&key) {
                    state.cleared_by_ancestor = true;
                    if ancestor == rel {
                        state.exact_markers.push(kind);
                    }
                }
            }
        }

        state
    }
}

#[derive(Default)]
pub struct EffectiveState {
    pub cleared_by_ancestor: bool,
    pub exact_markers: Vec<Marker>,   // typical: 0-1 entries
}

pub fn package_view_with_overlay(
    base: &crate::raw_view::RawView,
    overlay: &MarkOverlay<'_>,
    mode: crate::tree::ViewMode,
) -> crate::web::render::FlaggedView {
    // Mirror the shape of crate::web::render::package_view, but for each
    // folder consult `overlay.effective_state(root_idx, &folder.rel_path)`
    // before recording its `missing_ebook` and `cover_files`.
    ...
}
```

**Allocation note.** `effective_state`'s inner loop currently builds a `String` for `ancestor_rel.to_owned()` per probe to make the tuple key. Two valid optimizations exist but neither is in scope for first release: (a) a `Borrow`-friendly key wrapper so the probe uses `&str` without owning; (b) a custom `Hash + Eq` newtype keyed by `(usize, &str, Marker)`. The audit's depth assumption (2-3) keeps the per-folder probe count small (~4-6 allocations), and the equivalence test pins behavior either way.

**Marker enumeration.** `Marker::ALL` is a slice constant on the enum. If it doesn't exist yet, add it; the change is one line in `src/marker.rs` and is internal to that module.

### Render path

`package_view_with_overlay` walks `base` once, building the same `FlaggedView` shape that `package_view` produces. For each `ScannedFolder` in each `RootScan::Walked`:

- Consult `overlay.effective_state(root_idx, &folder.rel_path)`.
- If `cleared_by_ancestor`, the synthesized folder has `missing_ebook = false`; otherwise it inherits `folder.missing_ebook`.
- The synthesized `cover_files` is `folder.cover_files.clone()` plus `state.exact_markers.iter().map(Marker::filename).collect::<Vec<_>>()`, deduped (the production `add_marker` already dedupes; we mirror that here to keep `cover_files` byte-identical to the production path's output for the same logical state).
- All other fields (`rel_path`, `name`, the rest) are copied unchanged.

The rest of `package_view`'s shape (per-root structure, mode filtering, search-link wiring) is reproduced verbatim. The two functions are intentionally parallel; one diff to either is one diff to both. ADR-0024's per-section OOB swap path consumes `FlaggedView` and is unaffected.

`render_view` is the production renderer and is not modified.

## Mutex poison recovery

### `src/demo/state.rs`

```rust
impl DemoState {
    /// Acquire the session store lock, recovering on poison.
    ///
    /// Poison means a previous thread panicked while holding the lock; the
    /// session table itself is intact as far as the surviving thread can tell,
    /// so we proceed with a `tracing::warn` rather than propagate the panic.
    /// Mirrors `raw_view::lock_index` and `autosync::lock_inner`.
    pub(crate) fn lock_sessions(&self) -> std::sync::MutexGuard<'_, SessionStore> {
        self.sessions.lock().unwrap_or_else(|poison| {
            tracing::warn!("demo session mutex poisoned; recovering");
            poison.into_inner()
        })
    }
}
```

All five call sites in `src/demo/handlers.rs` (lines 163, 193, 237, 280) and `src/demo/state.rs:52` migrate from `state.sessions.lock().expect("session lock")` to `state.lock_sessions()`. The reaper task that runs on the `DEMO_IDLE_SECS` cadence migrates too.

## Binaries and CLI

### `Cargo.toml`

```toml
[features]
fixtures = []   # defined here, consumed by spec B (scenarios + synthetic gate)

[[bin]]
name = "missing-ebooks"
path = "src/main.rs"

[[bin]]
name = "missing-ebooks-demo"      # was: implicit "demo" from src/bin/demo.rs filename
path = "src/bin/demo.rs"
required-features = ["fixtures"]

[dependencies]
clap = { version = "4", features = ["derive"] }
```

Without `--features fixtures` the demo bin does not build. `cargo install missing-ebooks` installs only the production binary by default. To build the demo locally: `cargo run --features fixtures --bin missing-ebooks-demo`.

### `src/main.rs`

```rust
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "missing-ebooks",
    version,
    about = "Surface audiobook folders that are missing their ebooks.",
    after_help = "Environment variables: MISSING_EBOOKS_LIBRARY_ROOTS, \
                  MISSING_EBOOKS_BIND, MISSING_EBOOKS_PORT, MISSING_EBOOKS_CONFIG, \
                  MISSING_EBOOKS_LOG. See README for the full list."
)]
struct Cli {
    /// Print the resolved configuration as TOML and exit.
    #[arg(long)]
    print_config: bool,

    /// Path to a configuration file. Defaults to MISSING_EBOOKS_CONFIG or none.
    #[arg(long, value_name = "PATH")]
    config: Option<std::path::PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    if cli.print_config { print!("{}", config.to_toml_string()); return Ok(()); }
    // ... existing startup
}
```

`Config::load`'s signature already accepts an optional path. Env-var resolution stays inside `Config::load`. The `--print-config` flag preserves today's behavior exactly.

### `src/bin/demo.rs`

```rust
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "missing-ebooks-demo",
    version,
    about = "Run the public-facing demo with a synthetic library.",
    after_help = "Environment variables: DEMO_BIND, DEMO_SCENARIO, DEMO_MAX_SESSIONS, \
                  DEMO_IDLE_SECS, DEMO_COOKIE_NAME."
)]
struct Cli {
    /// Scenario name. Overrides DEMO_SCENARIO. Available: mixed-forest, messy-shelf,
    /// clean-error, root-flagged, pre-marked, big-library.
    #[arg(long)]
    scenario: Option<String>,
    /// Bind address. Overrides DEMO_BIND.
    #[arg(long)]
    bind: Option<String>,
}
```

The demo's existing env-only loader gains a thin layer: CLI flag value (if provided) wins over the env var. Cleaner than a fresh config plumbing pass; matches the existing precedence story.

## Tests

### Group 1: overlay correctness (load-bearing)

In `src/demo/overlay.rs`:

```rust
#[test]
fn overlay_matches_derive_view_byte_for_byte() {
    // For each scenario × each interesting mark set:
    //   - render via the old derive_view + package_view path (still present during migration)
    //   - render via package_view_with_overlay + render_view
    //   - assert byte-equality on the HTML output
    //
    // Scenarios: mixed-forest, messy-shelf, big-library, root-flagged, pre-marked.
    // Mark sets per scenario:
    //   - empty
    //   - single leaf
    //   - single root (rel == ".", ADR-0005)
    //   - ancestor + descendant (descendant is a no-op given ancestor cleared the subtree)
    //   - multi-root
    //   - all marker kinds on one folder
}
```

This is the merge gate for the migration commit (Commit 5 below). Once the test passes, `derive_view` is deleted in the same commit.

### Group 2: validation rejects unknown paths

In `src/demo/handlers.rs`:

```rust
#[test] async fn mark_rejects_unknown_root() { /* existing 400 path; reaffirm */ }
#[test] async fn mark_rejects_unknown_rel() { /* 400, body contains "unknown folder" */ }
#[test] async fn mark_accepts_root_dot_mark() { /* 200, ADR-0005 */ }
#[test] async fn unmark_rejects_unknown_root() { /* 400 */ }
#[test] async fn unmark_rejects_unknown_rel() { /* 400 */ }
#[test] async fn unmark_no_op_when_not_marked() { /* 200, set has no entry to remove */ }
```

### Group 3: storage semantics

In `src/demo/session.rs`:

```rust
#[test] fn insert_mark_dedupes() { /* same key twice → set has one entry, second returns Ok(false) */ }
#[test] fn marks_set_is_per_session() { /* two SessionIds, isolation */ }
#[test] fn clear_marks_empties_the_set() { /* unchanged behavior */ }
#[test] fn remove_mark_returns_whether_present() { /* Ok(true), Ok(false) */ }
```

### Group 4: demo undo round trip

In `src/demo/handlers.rs`. Mirrors `web/render.rs:1789` (`render_is_byte_equal_across_hits_and_a_mark_undo_round_trip`):

```rust
#[test]
async fn mark_then_unmark_round_trip_renders_pre_mark_state() {
    // POST /mark on a flagged folder.
    // Capture rendered section HTML.
    // POST /unmark on the same folder.
    // Capture rendered section HTML.
    // Assert second matches a fresh derivation with no marks, byte-for-byte.
}
```

### Group 5: poison recovery

In `src/demo/state.rs`. Mirrors `src/autosync.rs:482-512`:

```rust
#[test]
fn lock_sessions_recovers_from_poison() {
    let state = test_demo_state();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = state.lock_sessions();
        panic!("simulated panic under lock");
    }));
    let guard = state.lock_sessions();   // does not panic
    drop(guard);
}
```

### Not added

- No new criterion bench. The mechanism change shrinks per-request cost from `O((M+1) × F)` to `O(F × depth)` by construction. The overlay-correctness test pins output. A bench would only re-state what the mechanism guarantees. If a follow-up wants a number, lift the shape from `benches/render.rs`.
- No proptest. The equivalence test against `derive_view` is an oracle that exercises the interesting mark-set space enumeratively. Property-based generation would add CI variance without raising confidence above what the oracle already provides.
- No new SSE/integration test. The existing `tests/sse_demo_snapshot_only.rs` exercises `/events` against the demo router; the mark/unmark path's behavior at the SSE boundary doesn't change (the per-tick autosync loop is untouched).

## Migration order

Five commits, each leaving the tree in a passing state.

1. **`chore(cargo): rename demo binary to missing-ebooks-demo, declare fixtures feature`**
   - `Cargo.toml`: `[[bin]] name = "missing-ebooks-demo" required-features = ["fixtures"]`; `[features] fixtures = []`.
   - No source changes.
   - Verify: `cargo build --features fixtures --bins` produces both binaries with the new name.

2. **`feat(cli): add --help and --version to both binaries via clap`**
   - `Cargo.toml`: `clap = { version = "4", features = ["derive"] }`.
   - `src/main.rs`, `src/bin/demo.rs`: `Cli` structs, `Cli::parse()` at the top.
   - Behavior preserved: env vars and `--print-config` still work; CLI flags layer on.
   - Verify: `cargo run --bin missing-ebooks -- --help` and `cargo run --features fixtures --bin missing-ebooks-demo -- --help` both exit 0 with usage.

3. **`feat(demo): recover demo session mutex from poison`**
   - `src/demo/state.rs`: `lock_sessions` helper.
   - `src/demo/handlers.rs` + `src/demo/state.rs`: five call sites migrate.
   - Test: Group 5.
   - Smallest possible diff for Critical #1; lands independent of the storage rework.

4. **`refactor(demo): store marks as a set, validate folder existence at /mark`**
   - `src/demo/session.rs`: `Vec<Mark>` → `HashSet<MarkKey>`, new `SessionStore` API.
   - `src/demo/handlers.rs`: `folder_exists_in_base`, validation in `/mark`, set-shaped `insert_mark` callers.
   - `derive_view` still present and still functional: the handler builds a transient `Vec<Mark>` from the set so the old call shape works during the migration window.
   - Tests: Groups 2 + 3.
   - Verify: existing handler tests pass; new validation tests pass.

5. **`feat(demo): render via MarkOverlay, drop derive_view, add /unmark route`**
   - `src/demo/overlay.rs` (new): `MarkOverlay`, `package_view_with_overlay`, `EffectiveState`.
   - `src/demo/handlers.rs`: `derive_view` deleted; `render_with_overlay` used; `/unmark` handler added; router gains the route.
   - Tests: Groups 1 + 4.
   - This is the load-bearing commit. The Group 1 equivalence test is the merge gate.

Rough size: ~600-900 LOC diff including tests. ~250 LOC of new code in `src/demo/overlay.rs`; the rest is shaped substitution, validation, the `/unmark` handler, and the test suite.

## Risk

The Group 1 equivalence test is designed to fail loudly if `package_view_with_overlay` produces a `FlaggedView` whose serialized HTML differs from the old path's for the same logical state. If it fails on a scenario / mark-set combination, the resolution is one of:

- An overlay query missed an ancestor case (fix in `effective_state`).
- A field of `ScannedFolder` is read by the renderer and was not faithfully reproduced (fix in `package_view_with_overlay`'s synthesis).
- `apply_mark_raw` has a side effect not captured by the overlay model (analyze and decide whether the side effect was load-bearing or incidental).

If a category-3 surprise appears, the spec is wrong and we revisit. The other two are bug fixes within the spec.

Commits 1-4 are independent of Commit 5's risk. If Commit 5 needs to defer, the release-blockers spec can still ship #1, #2 (rename), #3 (clap), and the poison fix as a partial landing; the demo's `derive_view` would survive until a follow-up. That fallback is not the plan — the equivalence test is straightforward and the design is small — but it exists.

## Out of scope

- The `fixtures` feature gating of `scenarios.rs` and `synthetic.rs` themselves. Spec B owns that work. This spec only declares the feature placeholder so the demo bin can require it.
- Production CLI flag surface beyond `--help`, `--version`, `--print-config`, `--config`. If a future user wants `--bind` or `--library-roots` as flag overrides, that is a follow-up; the env-var path covers every deployment path the README documents today.
- Per-session memory metrics or quotas. The set is structurally bounded; no observability is added in this spec. If `DEMO_MAX_SESSIONS=1000` deployments ever sit close to the 90 MB worst-case ceiling in practice, a metric can land in spec H or a perf follow-up.
- A precomputed `HashSet<(usize, PathBuf)>` lookup table for `folder_exists_in_base`. Today's `O(F)` linear scan is sub-millisecond on the biggest scenario.
- Asset / static-file caching or fingerprinting. Demo asset routes are unchanged.
