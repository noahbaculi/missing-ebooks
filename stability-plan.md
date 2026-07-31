# v1 stability fixes implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the fifteen surviving findings from the 2026-07-30 stability audit: the store concurrency rework, request limiting, partial-scan warnings, four trivial fixes, and a nine-entry pin batch.

**Architecture:** `RawViewStore`'s internals move behind one `Arc<StoreInner>` so every mutation (mark, unmark, rescan, cold build) runs its side-effect-plus-bookkeeping sequence in a `tokio::spawn` task the handler merely awaits. The cache slot gains a monotonic generation so a build that began before an edit refuses to overwrite it (newest write wins). The router gains a global concurrency cap, the store a rescan cooldown, and the scanner a skipped-directory count plus depth cap that surface as a per-root warning strip.

**Tech Stack:** Rust (axum 0.8, tokio, tower 0.5, maud), cargo-nextest via `mise run test`, in-module unit tests plus `tower::ServiceExt::oneshot` router tests.

**Spec:** `docs/superpowers/specs/2026-07-30-v1-stability-fixes-design.md`. **Ledger:** `.scratch/v1-stability/FINDINGS.md` (annotate each finding with its commit hash as it lands; the file is gitignored, never commit it).

## Global constraints

- Concurrency cap: **16** concurrent requests on `router()`, via `tower::limit::GlobalConcurrencyLimitLayer`. Excess requests **queue rather than erroring**. No request timeout layer (decided against in the spec: a timeout is the only guardrail that can kill a legitimate cold-scan page load on a slow network mount).
- Rescan cooldown: **5 seconds**. Inside the window: skip the index clear, join the in-flight or fresh build via `build_coalesced`, return normally. Silent coalescing, no error UI.
- Depth cap: **64**, a `ScanSettings` constant named `MAX_DEPTH`, carrying a `// ponytail:` comment naming the ceiling and the upgrade path.
- Warning strip copy (count-only, pluralized): `N folders couldn't be read; results for this root may be incomplete.` (singular: `1 folder couldn't be read; ...`). Skipped paths stay in the server log only.
- `tower` moves from `[dev-dependencies]` to `[dependencies]` with `features = ["limit"]`; it is already in the tree via axum, so no new crate enters the graph. Keep `features = ["util"]` available to tests via a `[dev-dependencies]` entry.
- Commits: Conventional Commits on `main`, `fix(scope)` with a why-body for defects, `test(scope)` for pins, `refactor`/`docs`/`chore` where they fit. Granular, never squashed, never `--no-verify`, no attribution trailers, no ticket IDs in code comments.
- Run `mise run check` before each task's final commit. The pre-commit hook runs fmt, clippy, `cargo doc -D warnings`, and (for asset changes) `mise run test:accent`.
- Markdown prose (ADRs, CONTEXT.md) is written as unwrapped lines: one paragraph per line.
- After each commit, append a line `Landed in <short-hash>.` to the matching finding entry in `.scratch/v1-stability/FINDINGS.md`.
- Out of scope: release mechanics, public docs rework, new features, coverage percentages, fuzzing, the wontfix findings (F8 code fix, F9, F22, F23, F24), and F28 (user merges PRs 4-7 from the GitHub web UI).

## File structure

| File | Change | Tasks |
| --- | --- | --- |
| `src/state.rs` | `StoreInner` extraction, slot generation, spawned mutations, `write_mark` restructure, rescan cooldown, F15 mislabel fix, pins | 1-5, 7, 10, 11 |
| `src/web.rs` | Concurrency layer, cap test, partial-scan e2e test, file-root banner pin | 6, 9, 11 |
| `src/scanner.rs` | `dirs_skipped` counting, `MAX_DEPTH` cap, `RootScan::Walked.skipped_dirs`, pins | 8, 11 |
| `src/web/render.rs` | Warning strip, coverage-floor pin, actionability pin | 9, 10, 12 |
| `src/web/assets.rs` | Client-contract pins (view key, toasts, poll gating) | 12 |
| `assets/app.css` | `.alert-warning` style | 9 |
| `src/shutdown.rs` | Match the `ctrl_c()` results at all three sites | 10 |
| `src/config.rs` | `ConfigError::Read` pin | 11 |
| `src/synthetic.rs`, `src/tree.rs` | `skipped_dirs` field ripple | 8 |
| `Cargo.toml` | tower dependency move, `undocumented_unsafe_blocks` lint | 6, 10 |
| `CONTEXT.md` | Coverage formula amend | 10 |
| `docs/adr/0002-*.md`, `docs/adr/0027-*.md` | Amendment notes | 5 |
| `docs/adr/0036-*.md`, `docs/adr/0037-*.md` | New ADRs | 5, 7 |

Tasks 1-4 are sequential (each builds on the previous store shape). Task 5 documents them. Tasks 6-7 (cluster 3), 8-9 (cluster 4), 10 (trivial), and 11-12 (pins) follow in that order, matching the ledger's ranking.

---

### Task 1: Move the store internals behind `Arc<StoreInner>`

Pure refactor, no behavior change. Later tasks spawn closures that must own the cache slot, the inflight lock, and the scan substrate, so everything moves behind one `Arc`.

**Files:**
- Modify: `src/state.rs`

**Interfaces:**
- Consumes: the current `RawViewStore` (fields `cache`, `inflight`, `rebuild_count`, `ttl`, `settings`, `dir_indices`, `config` at `src/state.rs:48-77`).
- Produces: `pub struct RawViewStore { inner: Arc<StoreInner> }`; `struct StoreInner` holding all seven fields; `impl StoreInner` methods `fn load(&self)`, `fn slot(&self)`, `fn store_fresh(&self, raw: Arc<RawView>) -> Arc<RawView>`, `fn invalidate_index(&self, root: usize, canonical_root: &Path, rel: &str)`; associated fns `async fn current(this: &Arc<StoreInner>) -> Arc<RawView>` and `async fn build_coalesced(this: &Arc<StoreInner>, recheck_fresh: bool) -> Arc<RawView>`. `RawViewStore`'s public surface (`new`, `current`, `rescan`, `write_mark`, `remove_mark`, `rebuild_count`, `dir_index_len_for_test`) is unchanged in signature.

- [ ] **Step 1: Restructure the types**

In `src/state.rs`, replace the `RawViewStore` struct with a thin wrapper plus an inner struct. Keep every existing field doc comment on the moved field.

```rust
/// Owns the scan substrate, the TTL-bounded cache slot, and the marker file
/// IO. The single place where raw scan output is produced, memoized, and
/// edited. See ADR-0027.
pub struct RawViewStore {
    /// Shared internals, behind one `Arc` so mutation sequences can run in
    /// spawned tasks that outlive a dropped request handler
    inner: Arc<StoreInner>,
}

/// The store's fields and lock protocol. `RawViewStore` methods delegate
/// here; spawned tasks capture an `Arc<StoreInner>` clone
struct StoreInner {
    cache: std::sync::RwLock<Option<Arc<CacheEntry>>>,
    inflight: Mutex<Option<Weak<SharedBuild>>>,
    rebuild_count: AtomicU64,
    ttl: Option<Duration>,
    settings: Arc<ScanSettings>,
    dir_indices: Vec<Arc<DirIndex>>,
    config: Arc<Config>,
}
```

Each field keeps its existing doc comment from `src/state.rs:49-77`, moved verbatim.

- [ ] **Step 2: Move the methods**

Mechanical moves, bodies unchanged except `self.<field>` becomes `this.<field>` in the two associated fns:

- `load`, `slot`, `store_fresh`, `invalidate_index` become `&self` methods on `StoreInner`.
- `current` and `build_coalesced` become associated fns on `StoreInner` taking `this: &Arc<StoreInner>` (Rust does not allow `self: &Arc<Self>`, so they are associated fns; they need the `Arc` in Task 2 to spawn the build task).
- `RawViewStore::new` builds `inner: Arc::new(StoreInner { ... })`.
- `RawViewStore::current` becomes `StoreInner::current(&self.inner).await`; `rescan` iterates `self.inner.dir_indices` then calls `StoreInner::build_coalesced(&self.inner, false).await`.
- `write_mark` and `remove_mark` stay on `RawViewStore` with every `self.x` field access rewritten to `self.inner.x` (including the `build_view(self.inner.config.as_ref(), &self.inner.settings, &self.inner.dir_indices)` cold arm and both `self.inner.inflight.lock()` sites).
- `rebuild_count` and `dir_index_len_for_test` read through `self.inner`.

- [ ] **Step 3: Fix the tests**

In `src/state.rs::tests`, update direct field pokes (the tests live in the same module, so private access is fine):

- `state.store.ttl` → `state.store.inner.ttl` (two tests).
- `store.dir_indices[0]` → `store.inner.dir_indices[0]` (five tests: `store_rescan_clears_the_dir_index_then_repopulates_it`, `store_ttl_zero_keeps_the_dir_index_warm`, `store_write_mark_invalidates_the_marked_dir_in_the_index`, `store_write_mark_at_the_root_invalidates_the_root_in_the_index`, and the two-root test's construction is untouched).
- `store.build_coalesced(true)` / `store.build_coalesced(false)` → `StoreInner::build_coalesced(&store.inner, true)` / `(..., false)` (two tests).

- [ ] **Step 4: Run the full test suite**

Run: `mise run test`
Expected: PASS, no behavior change.

- [ ] **Step 5: Check and commit**

Run `mise run check`, then:

```bash
git add src/state.rs
git commit -m "refactor(state): move store internals behind an Arc

Spawned mutation tasks (next commits) must own the cache slot, the
inflight lock, and the scan substrate past a dropped handler future.
No behavior change."
```

---

### Task 2: Generation-guard the slot and let the build publish its own result (F4, F11)

The cache slot gains a monotonic generation. Every store and every in-place edit bumps it. The coalesced build records the generation at registration, runs in a `tokio::spawn` task, and its store becomes compare-and-store: if the generation moved during the walk, the result is served to awaiters but not persisted. Because the build task is spawned, an aborted first request no longer discards the completed walk.

**Files:**
- Modify: `src/state.rs`

**Interfaces:**
- Consumes: Task 1's `StoreInner` and its associated fns.
- Produces: `struct Slot { generation: u64, entry: Option<Arc<CacheEntry>> }` behind `cache: std::sync::RwLock<Slot>`; `StoreInner` methods `fn generation(&self) -> u64`, `fn store_if_unchanged(&self, expected: u64, raw: &Arc<RawView>)`, `fn edit_fresh(&self, edit: impl FnOnce(&mut RawView)) -> Option<Arc<RawView>>`, and a temporary unconditional `fn store_fresh(&self, raw: Arc<RawView>) -> Arc<RawView>` (deleted in Task 3). Test seams: `pub(crate) struct BuildGate { entered: Arc<Semaphore>, release: Arc<Semaphore> }` with `BuildGate::new()`, `RawViewStore::set_build_gate(&self, gate: BuildGate)` (`#[cfg(test)]`, `pub(crate)` because Task 6's router test uses it), and `StoreInner::pause_at_build_gate(&self)`.

- [ ] **Step 1: Write the two failing tests**

In `src/state.rs::tests` (they will not compile yet; that counts as failing, the point is writing the behavior first):

```rust
#[tokio::test]
async fn a_mark_landing_mid_build_survives_the_builds_store() {
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
    let _warm = store.current().await;

    let gate = BuildGate::new();
    store.set_build_gate(gate.clone());

    // A forced rebuild pauses between its walk and its store
    let inner = Arc::clone(&store.inner);
    let build = tokio::spawn(async move { StoreInner::build_coalesced(&inner, false).await });
    gate.entered.acquire().await.unwrap().forget();

    // The mark lands while the walk's result is in hand but unstored
    let applied = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
    assert!(applied.created);

    gate.release.add_permits(1);
    let built = build.await.unwrap();
    assert!(book_missing(&built), "awaiters still receive the pre-mark walk");

    let served = store.current().await;
    assert!(
        !book_missing(&served),
        "the interleaved mark survives the build's store"
    );
}

#[tokio::test]
async fn an_aborted_owner_still_stores_the_completed_build() {
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
    let gate = BuildGate::new();
    store.set_build_gate(gate.clone());

    let inner = Arc::clone(&store.inner);
    let owner = tokio::spawn(async move { StoreInner::build_coalesced(&inner, true).await });
    gate.entered.acquire().await.unwrap().forget();

    // The only awaiter vanishes mid-build
    owner.abort();
    let _ = owner.await;

    gate.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.rebuild_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached build task never stored its result");

    let _ = store.current().await;
    assert_eq!(
        store.rebuild_count(),
        1,
        "the follow-up read serves the stored slot without a second walk"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run a_mark_landing_mid_build an_aborted_owner`
Expected: compile error (`BuildGate` and `set_build_gate` do not exist yet). After Step 3's seams exist but before Step 4's logic, `a_mark_landing_mid_build_survives_the_builds_store` fails on the final assert (the build's store clobbers the mark) and `an_aborted_owner_still_stores_the_completed_build` times out (the aborted owner never stores).

- [ ] **Step 3: Add the `Slot`, the gate seam, and the helpers**

```rust
/// The guarded cache cell: the entry plus a monotonically increasing
/// generation. Every store and every in-place edit bumps the generation, so
/// a build that began before the latest write detects the move and declines
/// to overwrite it (newest write wins, ADR-0036)
struct Slot {
    generation: u64,
    entry: Option<Arc<CacheEntry>>,
}
```

In `StoreInner`: `cache: std::sync::RwLock<Slot>` (constructed as `Slot { generation: 0, entry: None }`), plus a `#[cfg(test)] build_gate: std::sync::Mutex<Option<BuildGate>>` field (constructed with `#[cfg(test)] build_gate: std::sync::Mutex::new(None),` in `RawViewStore::new`).

```rust
/// Test-only pause point between a cold build's walk and its store, so
/// interleaving tests can act while the result is in hand but unpublished
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct BuildGate {
    /// The build adds one permit here when it reaches the gate
    pub(crate) entered: Arc<tokio::sync::Semaphore>,
    /// The build consumes one permit from here before storing
    pub(crate) release: Arc<tokio::sync::Semaphore>,
}

#[cfg(test)]
impl BuildGate {
    pub(crate) fn new() -> BuildGate {
        BuildGate {
            entered: Arc::new(tokio::sync::Semaphore::new(0)),
            release: Arc::new(tokio::sync::Semaphore::new(0)),
        }
    }
}
```

On `RawViewStore`:

```rust
/// Arm the test-only build gate. Every later cold build pauses at it
#[cfg(test)]
pub(crate) fn set_build_gate(&self, gate: BuildGate) {
    *self
        .inner
        .build_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate);
}
```

On `StoreInner` (alongside `load` and `slot`, which now read and write `Slot`; `load` returns `guard.entry.clone()`):

```rust
/// Current slot generation. Read under the inflight lock at build
/// registration so the pairing with `store_if_unchanged` is race-free
fn generation(&self) -> u64 {
    self.cache
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .generation
}

/// Store a freshly built raw view unless the slot moved since `expected`
/// was read. A mark or undo that landed mid-walk already bumped the
/// generation, and newer data must not be overwritten by an older walk.
/// Bumps `rebuild_count` only when the store happens
fn store_if_unchanged(&self, expected: u64, raw: &Arc<RawView>) {
    let mut slot = self.slot();
    if slot.generation != expected {
        tracing::debug!("discarding a stale build; the slot moved during the walk");
        return;
    }
    slot.generation += 1;
    slot.entry = Some(Arc::new(CacheEntry {
        stored_at: Instant::now(),
        raw: Arc::clone(raw),
    }));
    self.rebuild_count.fetch_add(1, Ordering::Relaxed);
}

/// Clone-edit-store the fresh entry under the write lock, preserving
/// `stored_at` and bumping the generation. `None` when the slot is cold or
/// stale, leaving the caller to rebuild
fn edit_fresh(&self, edit: impl FnOnce(&mut RawView)) -> Option<Arc<RawView>> {
    let mut slot = self.slot();
    let entry = slot.entry.as_ref()?;
    if !is_fresh(entry, self.ttl) {
        return None;
    }
    let mut next = (*entry.raw).clone();
    edit(&mut next);
    let next = Arc::new(next);
    slot.generation += 1;
    slot.entry = Some(Arc::new(CacheEntry {
        stored_at: entry.stored_at,
        raw: Arc::clone(&next),
    }));
    Some(next)
}

/// Block at the armed build gate, if any. Compiled out of production
#[cfg(test)]
async fn pause_at_build_gate(&self) {
    let gate = self
        .build_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(gate) = gate {
        gate.entered.add_permits(1);
        gate.release
            .acquire()
            .await
            .expect("build gate semaphore closed")
            .forget();
    }
}
```

Keep a temporary unconditional `store_fresh` for `write_mark`'s private cold arm (Task 3 deletes it): same body as `store_if_unchanged` minus the generation compare, returning `raw`.

- [ ] **Step 4: Rework `build_coalesced` and the edit arms**

```rust
/// Start a build, or join the one already running. The walk and its store
/// run in a spawned task, so a dropped request cannot discard a completed
/// build. The store is generation-guarded: registered under the inflight
/// lock, compared at store time, skipped when an edit landed mid-walk
async fn build_coalesced(this: &Arc<StoreInner>, recheck_fresh: bool) -> Arc<RawView> {
    let handle = {
        let mut slot = this.inflight.lock().await;
        if recheck_fresh
            && let Some(entry) = this.load()
            && is_fresh(&entry, this.ttl)
        {
            return Arc::clone(&entry.raw);
        }
        if let Some(existing) = slot.as_ref().and_then(Weak::upgrade) {
            existing
        } else {
            let registered_at = this.generation();
            let task_inner = Arc::clone(this);
            let task = tokio::spawn(async move {
                let raw = Arc::new(
                    build_view(
                        task_inner.config.as_ref(),
                        &task_inner.settings,
                        &task_inner.dir_indices,
                    )
                    .await,
                );
                #[cfg(test)]
                task_inner.pause_at_build_gate().await;
                task_inner.store_if_unchanged(registered_at, &raw);
                raw
            });
            let build: SharedBuild = async move {
                // JoinError is unreachable in practice: per-root panics are
                // folded into RootScan::Failed by build_section
                task.await.expect("cold build task panicked")
            }
            .boxed()
            .shared();
            let handle = Arc::new(build);
            *slot = Some(Arc::downgrade(&handle));
            handle
        }
    };
    // Keep the Arc alive across the await so a concurrent caller can
    // upgrade the Weak and join this build
    (*handle).clone().await
}
```

The `if owns { store_fresh }` tail is gone. In `write_mark`, replace the fresh-arm clone-and-edit with `self.inner.edit_fresh(|next| apply_mark_raw(next, root, &rel_for_edit, marker))` (the cold arm keeps `self.inner.store_fresh(...)` for now). In `remove_mark`, replace the splice block with:

```rust
let spliced = {
    let _edit = self.inner.inflight.lock().await;
    self.inner.edit_fresh(|next| {
        if root < next.len() {
            next[root] = section;
        }
    })
};
```

- [ ] **Step 5: Run the new tests and the full suite**

Run: `cargo nextest run a_mark_landing_mid_build an_aborted_owner`
Expected: PASS.

Run: `mise run test`
Expected: PASS. The rebuild-count tests still hold: `store_if_unchanged` bumps the count exactly when a build stores, and warm edits never call it.

- [ ] **Step 6: Check and commit**

Run `mise run check`, then:

```bash
git add src/state.rs
git commit -m "fix(state): store builds from their own task, guarded by a slot generation

A build that began before a mark used to overwrite the slot with
pre-mark data after the mark completed, and stored_at = now kept the
stale view alive for a full TTL (F4). Only the owning caller stored a
completed build, so an aborted first request threw the walk away (F11).
The slot now carries a generation every store and edit bumps; the build
runs in a spawned task, records the generation at registration, and
declines to store when it moved. Awaiters still receive the built view
for rendering."
```

Annotate F4 and F11 in the ledger with the hash.

---

### Task 3: Restructure `write_mark` (F1, F12)

Mirror `remove_mark`: the inflight guard scopes to the load-edit-store only, the cold fallback routes through `build_coalesced(false)` so concurrent cold marks coalesce, and a re-mark (`created == false`) skips the clone-and-edit entirely.

**Files:**
- Modify: `src/state.rs`

**Interfaces:**
- Consumes: Task 2's `edit_fresh`, `StoreInner::build_coalesced`, `BuildGate`.
- Produces: the final pre-spawn `write_mark` body (Task 4 wraps it); `#[cfg(test)] fn peek_stored_arc(&self) -> Option<Arc<RawView>>` on `RawViewStore`. Deletes the temporary `store_fresh`.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn concurrent_cold_marks_coalesce_into_one_walk() {
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("A/01.mp3"));
    crate::scenarios::touch(&dir.path().join("B/01.mp3"));
    let store = Arc::new(test_store(
        Some(Duration::from_secs(600)),
        dir.path().to_path_buf(),
    ));
    let gate = BuildGate::new();
    store.set_build_gate(gate.clone());

    let s1 = Arc::clone(&store);
    let first = tokio::spawn(async move { s1.write_mark(0, "A", Marker::NoEbook).await });
    // The first cold mark's fallback build reaches the gate after its walk.
    // A timeout here is the old shape failing: the private build_view arm
    // never routes through the coalesced (gated) build
    tokio::time::timeout(Duration::from_secs(5), gate.entered.acquire())
        .await
        .expect("the cold mark never routed through the coalesced build")
        .unwrap()
        .forget();

    let s2 = Arc::clone(&store);
    let second = tokio::spawn(async move { s2.write_mark(0, "B", Marker::NoEbook).await });
    // Drive the second mark past its marker write and into the join
    tokio::time::timeout(Duration::from_secs(5), async {
        while !dir.path().join("B/.no_ebook").exists() {
            tokio::task::yield_now().await;
        }
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the second mark never wrote its marker");

    gate.release.add_permits(1);
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        store.rebuild_count(),
        1,
        "concurrent cold marks share one coalesced walk"
    );
}

#[tokio::test]
async fn a_remark_returns_the_cached_view_without_cloning_the_slot() {
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
    let _warm = store.current().await;
    let first = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
    assert!(first.created);
    let arc_before = store.peek_stored_arc().unwrap();
    let generation_before = store.inner.generation();

    let second = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
    assert!(!second.created);
    assert!(
        Arc::ptr_eq(&arc_before, &store.peek_stored_arc().unwrap()),
        "a re-mark must not clone and restore the slot"
    );
    assert_eq!(
        store.inner.generation(),
        generation_before,
        "a re-mark must not bump the slot generation"
    );
}
```

Add the seam the second test needs:

```rust
/// Test-only read of the stored raw slot, ignoring freshness
#[cfg(test)]
fn peek_stored_arc(&self) -> Option<Arc<RawView>> {
    self.inner.load().map(|entry| Arc::clone(&entry.raw))
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run concurrent_cold_marks a_remark_returns`
Expected: `concurrent_cold_marks_coalesce_into_one_walk` FAILS on the gate timeout (the old private `build_view` arm never routes through the coalesced, gated build). `a_remark_returns_the_cached_view_without_cloning_the_slot` FAILS on `Arc::ptr_eq` (the old body clones and restores even when `created == false`).

- [ ] **Step 3: Restructure `write_mark`**

Replace the body after the `write_marker` result match (keep the `BadRoot` bail, the `spawn_blocking` call, and the failure arm exactly as they are):

```rust
// A re-mark created nothing on disk: no index entry went stale and no
// cover-file list changed, so skip the clone-and-edit entirely (F12)
if !created {
    return Ok(Applied {
        raw: self.current().await,
        created,
    });
}
self.inner.invalidate_index(root, &canonical_root, rel);

// Mirror remove_mark: the inflight guard scopes to the load-edit-store
// only, never across a walk (F1)
let edited = {
    let _edit = self.inner.inflight.lock().await;
    self.inner
        .edit_fresh(|next| apply_mark_raw(next, root, rel, marker))
};
let raw = match edited {
    Some(next) => next,
    // Cold or stale slot: the marker is already on disk, so a coalesced
    // rebuild reflects it and concurrent cold marks share one walk
    None => StoreInner::build_coalesced(&self.inner, false).await,
};
Ok(Applied { raw, created })
```

The `rel_for_edit` clone and the old `let raw = { let _edit = ...; ... }` block that held the lock across `build_view` are gone. Delete the now-unused temporary `StoreInner::store_fresh`.

- [ ] **Step 4: Run the new tests and the full suite**

Run: `cargo nextest run concurrent_cold_marks a_remark_returns`
Expected: PASS.

Run: `mise run test`
Expected: PASS. `store_write_mark_idempotent_create_false_on_second_call`, `store_write_mark_on_a_cold_cache_scans_fresh`, and `store_write_mark_edits_the_slot_in_place` all still hold.

- [ ] **Step 5: Check and commit**

Run `mise run check`, then:

```bash
git add src/state.rs
git commit -m "fix(state): scope write_mark's guard to the edit and coalesce its cold build

An unauthenticated POST /mark on a cold or stale slot ran a full
multi-root walk while holding the store's coordination mutex, queueing
every other request behind it for the whole walk, and concurrent cold
marks serialized into N sequential walks (F1). The guard now scopes to
the load-edit-store and the fallback routes through build_coalesced,
mirroring remove_mark. A re-mark (created == false) short-circuits
before the O(library) clone-and-edit, so the replayable no-op mark no
longer deep-copies the view under the write lock (F12)."
```

The two findings share one restructure of one function body, so they land as one commit naming both. Annotate F1 and F12 in the ledger.

---

### Task 4: Run mutation bookkeeping to completion in spawned tasks (F3, F5, F10)

One idiom applied at the three remaining sites: `write_mark`, `remove_mark`, and `rescan` each move their whole side-effect-plus-bookkeeping sequence into a `tokio::spawn` task the handler awaits. A client disconnect that drops the handler future can no longer split a marker write or delete from its index invalidation and cache edit, or strand cleared indices behind an aborted rescan.

**Files:**
- Modify: `src/state.rs`

**Interfaces:**
- Consumes: Task 3's `write_mark` body, the existing `remove_mark` body, Task 1's `rescan`.
- Produces: `StoreInner` associated fns `async fn apply_write_mark(this: &Arc<StoreInner>, root_path: PathBuf, root: usize, rel: &str, marker: Marker) -> Result<Applied, WriteFailure>` and `async fn apply_remove_mark(this: &Arc<StoreInner>, root_path: PathBuf, root: usize, rel: &str, marker: Marker) -> Result<Arc<RawView>, WriteFailure>`. Public signatures unchanged.

- [ ] **Step 1: Write the three failing tests**

```rust
#[tokio::test]
async fn a_dropped_mark_still_lands_its_bookkeeping() {
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
    let _warm = store.current().await;
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    assert!(
        store.inner.dir_indices[0]
            .get_cloned(&canonical.join("Book"))
            .is_some()
    );

    {
        // One poll starts the spawned task, then the handler-side future
        // drops, standing in for a client disconnect
        let fut = store.write_mark(0, "Book", Marker::NoEbook);
        let mut fut = std::pin::pin!(fut);
        let _ = tokio::time::timeout(Duration::from_millis(0), fut.as_mut()).await;
    }

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let marker_on_disk = dir.path().join("Book/.no_ebook").exists();
            let index_invalidated = store.inner.dir_indices[0]
                .get_cloned(&canonical.join("Book"))
                .is_none();
            let view_edited = store
                .peek_stored_arc()
                .is_some_and(|raw| !book_missing(&raw));
            if marker_on_disk && index_invalidated && view_edited {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the spawned mark task never finished its bookkeeping");
}

#[tokio::test]
async fn a_dropped_unmark_still_lands_its_bookkeeping() {
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
    let _warm = store.current().await;
    let _ = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();

    {
        let fut = store.remove_mark(0, "Book", Marker::NoEbook);
        let mut fut = std::pin::pin!(fut);
        let _ = tokio::time::timeout(Duration::from_millis(0), fut.as_mut()).await;
    }

    // The splice re-lists Book into the index, so assert on the two ends:
    // the file is gone and the stored view re-flags the folder
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let marker_gone = !dir.path().join("Book/.no_ebook").exists();
            let view_reflagged = store
                .peek_stored_arc()
                .is_some_and(|raw| book_missing(&raw));
            if marker_gone && view_reflagged {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the spawned unmark task never finished its bookkeeping");
}

#[tokio::test]
async fn a_dropped_rescan_still_repopulates_the_index() {
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
    let _warm = store.current().await;
    let before = store.rebuild_count();

    {
        let fut = store.rescan();
        let mut fut = std::pin::pin!(fut);
        let _ = tokio::time::timeout(Duration::from_millis(0), fut.as_mut()).await;
    }

    tokio::time::timeout(Duration::from_secs(5), async {
        while store.rebuild_count() == before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the spawned rescan task never rebuilt");
    assert!(
        store.inner.dir_indices[0].len() > 0,
        "the aborted rescan left a repopulated index, not a stranded empty one"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run a_dropped_mark a_dropped_unmark a_dropped_rescan`
Expected: all three time out. Dropping the future after one poll cancels the inline sequence; the marker IO on the blocking pool may land, but the bookkeeping after the first `.await` never runs.

- [ ] **Step 3: Move each body into a spawned task**

Extract the current `write_mark` body (everything after the `BadRoot` bail) into an associated fn on `StoreInner`. In full:

```rust
/// The whole mark sequence: guard-and-write on the blocking pool, index
/// invalidation, cache edit or coalesced rebuild. Runs inside a spawned
/// task so a dropped handler cannot split the write from its bookkeeping
async fn apply_write_mark(
    this: &Arc<StoreInner>,
    root_path: PathBuf,
    root: usize,
    rel: &str,
    marker: Marker,
) -> Result<Applied, WriteFailure> {
    let rel_owned = rel.to_string();
    let io = tokio::task::spawn_blocking(move || write_marker(&root_path, &rel_owned, marker))
        .await
        .map_err(|_| WriteError::WriteFailed(std::io::Error::other("marker write task failed")))
        .and_then(|io| io);
    let (created, canonical_root) = match io {
        Ok(pair) => pair,
        Err(error) => {
            // The write failed. Hand back the still-valid view so the
            // caller renders an inline alert without a second store hop
            let raw = StoreInner::current(this).await;
            return Err(WriteFailure::Failed { error, raw });
        }
    };
    // A re-mark created nothing on disk: no index entry went stale and no
    // cover-file list changed, so skip the clone-and-edit entirely (F12)
    if !created {
        return Ok(Applied {
            raw: StoreInner::current(this).await,
            created,
        });
    }
    // A self-write may not bump the folder mtime, so force a re-list
    this.invalidate_index(root, &canonical_root, rel);

    // Mirror remove_mark: the inflight guard scopes to the load-edit-store
    // only, never across a walk (F1)
    let edited = {
        let _edit = this.inflight.lock().await;
        this.edit_fresh(|next| apply_mark_raw(next, root, rel, marker))
    };
    let raw = match edited {
        Some(next) => next,
        // Cold or stale slot: the marker is already on disk, so a coalesced
        // rebuild reflects it and concurrent cold marks share one walk
        None => StoreInner::build_coalesced(this, false).await,
    };
    Ok(Applied { raw, created })
}
```

And `remove_mark`'s body as `apply_remove_mark`, also in full:

```rust
/// The whole undo sequence: guard-and-delete on the blocking pool, index
/// invalidation, per-root rewalk, splice or coalesced rebuild. Spawned for
/// the same reason as apply_write_mark
async fn apply_remove_mark(
    this: &Arc<StoreInner>,
    root_path: PathBuf,
    root: usize,
    rel: &str,
    marker: Marker,
) -> Result<Arc<RawView>, WriteFailure> {
    let rel_owned = rel.to_string();
    let delete_path = root_path.clone();
    let io = tokio::task::spawn_blocking(move || delete_marker(&delete_path, &rel_owned, marker))
        .await
        .map_err(|_| WriteError::WriteFailed(std::io::Error::other("marker delete task failed")))
        .and_then(|io| io);
    let canonical_root = match io {
        Ok(path) => path,
        Err(error) => {
            let raw = StoreInner::current(this).await;
            return Err(WriteFailure::Failed { error, raw });
        }
    };

    // A self-delete may not bump the folder mtime, so force a re-list
    this.invalidate_index(root, &canonical_root, rel);

    // Walk the one affected root BEFORE taking the lock. ADR-0027 keeps
    // the inflight lock held for microseconds only, and the dir index is
    // warm, so this is the cheap re-list of that subtree
    let section = build_section(
        root_path,
        Arc::clone(&this.settings),
        Arc::clone(&this.dir_indices[root]),
    )
    .await;

    let spliced = {
        let _edit = this.inflight.lock().await;
        this.edit_fresh(|next| {
            if root < next.len() {
                next[root] = section;
            }
        })
    };
    match spliced {
        Some(raw) => Ok(raw),
        // Cold or stale slot: rebuild through the single-flight coalescer
        // with no lock held across the walk
        None => Ok(StoreInner::build_coalesced(this, false).await),
    }
}
```

The public methods become spawn-and-await wrappers:

```rust
pub(crate) async fn write_mark(
    &self,
    root: usize,
    rel: &str,
    marker: Marker,
) -> Result<Applied, WriteFailure> {
    let root_path = self
        .inner
        .config
        .library_roots
        .get(root)
        .ok_or(WriteFailure::BadRoot)?
        .clone();
    let inner = Arc::clone(&self.inner);
    let rel = rel.to_string();
    // Spawned so a client disconnect cannot split the marker write from
    // its index invalidation and cache edit (ADR-0036)
    match tokio::spawn(
        async move { StoreInner::apply_write_mark(&inner, root_path, root, &rel, marker).await },
    )
    .await
    {
        Ok(result) => result,
        Err(join_err) => {
            tracing::error!(error = %join_err, "mark task panicked");
            let raw = StoreInner::current(&self.inner).await;
            Err(WriteFailure::Failed {
                error: WriteError::WriteFailed(std::io::Error::other("mark task failed")),
                raw,
            })
        }
    }
}
```

`remove_mark` mirrors it (log message "unmark task panicked", error string "unmark task failed"). `rescan` becomes:

```rust
/// Force a cold scan: clear every per-root index, then rebuild. Ignores
/// the TTL. Spawned so an aborted request cannot strand cleared indices
/// with nothing to refill them
pub(crate) async fn rescan(&self) -> Arc<RawView> {
    let inner = Arc::clone(&self.inner);
    tokio::spawn(async move {
        for index in &inner.dir_indices {
            index.clear();
        }
        StoreInner::build_coalesced(&inner, false).await
    })
    .await
    .expect("rescan task panicked")
}
```

The old inline `spawn_blocking` JoinError mapping inside the bodies stays as is; the new outer JoinError arms only cover a panic in the bookkeeping itself.

- [ ] **Step 4: Run the new tests and the full suite**

Run: `cargo nextest run a_dropped_mark a_dropped_unmark a_dropped_rescan`
Expected: PASS.

Run: `mise run test`
Expected: PASS.

- [ ] **Step 5: Check and commit**

Run `mise run check`, then:

```bash
git add src/state.rs
git commit -m "fix(state): run mutation bookkeeping to completion in spawned tasks

axum drops the handler future on client disconnect. An aborted unmark
deleted the marker while the stale index kept the gap hidden (F3), an
aborted mark wrote the marker but skipped the index invalidation and
cache edit (F5), and an aborted rescan cleared every index before the
only await that repopulates it (F10). Each sequence now runs in a
tokio::spawn task the handler awaits, so once started it completes
whether or not the request survives. One idiom, three sites; the cold
build got the same treatment in the generation-guard commit."
```

Annotate F3, F5, and F10 in the ledger.

---

### Task 5: Reconcile the docs with the new concurrency model

The invariant comment at the inflight field and ADR-0002/ADR-0027 still promise a serialization the code no longer provides that way. Update them and record the new model as ADR-0036.

**Files:**
- Modify: `src/state.rs` (comments only), `docs/adr/0002-marker-writes-edit-cache-in-place.md`, `docs/adr/0027-substrate-consolidated-behind-rawviewstore.md`
- Create: `docs/adr/0036-store-mutations-spawned-and-generation-guarded.md`

**Interfaces:**
- Consumes: the shipped behavior from Tasks 1-4.
- Produces: prose only. No code change.

- [ ] **Step 1: Rewrite the inflight field doc**

Replace the comment on `StoreInner.inflight` (originally `src/state.rs:54-59`):

```rust
    /// Holds only the in-flight build handle (as a `Weak`, so it expires
    /// when the last awaiter drops it). Held for microseconds to register
    /// or join a build, never across the walk. Also serializes the
    /// load-edit-store of marker edits against each other. Ordering against
    /// a concurrent build's store is the slot generation's job, not this
    /// lock's (see ADR-0036)
    inflight: Mutex<Option<Weak<SharedBuild>>>,
```

Sweep the rest of `src/state.rs` for stale claims: the module doc's ADR-0002 sentence stays (the edit-in-place decision still holds), but any comment saying the inflight lock serializes an edit against a build's store must now cite the generation instead.

- [ ] **Step 2: Amend the two ADRs**

In `docs/adr/0002-marker-writes-edit-cache-in-place.md`, below the existing amendment blockquote, add (as one unwrapped line):

```markdown
> Amended 2026-07-30 by ADR-0036: the "one lock serializes every cache mutation" protocol below was replaced by a slot generation (newest write wins) and mutations that run to completion in spawned tasks.
```

In `docs/adr/0027-substrate-consolidated-behind-rawviewstore.md`, add directly under the title:

```markdown
> Amended 2026-07-30 by ADR-0036: the store's internals moved behind `Arc<StoreInner>`, the coalesced build publishes its own result from a spawned task, and slot stores are generation-guarded.
```

- [ ] **Step 3: Write ADR-0036**

Create `docs/adr/0036-store-mutations-spawned-and-generation-guarded.md` with this content (each paragraph one unwrapped line):

```markdown
# Store mutations run in spawned tasks and the slot is generation-guarded

Date: 2026-07-30.

## Context

axum drops a handler future when its client disconnects. Every store mutation (`write_mark`, `remove_mark`, `rescan`, and the cold build inside `build_coalesced`) used to run its side effect and its bookkeeping inline in the handler future, so an abort could split a marker write or delete from its index invalidation and cache edit, strand cleared indices behind an aborted rescan, or discard a completed walk whose owning request vanished. Separately, ADR-0027's microsecond-lock rework moved the build's store outside the inflight lock without revisiting ADR-0002's claim that one lock serializes every cache mutation, so a build that started before a mark could overwrite the slot with pre-mark data after the mark completed, and the refreshed `stored_at` kept the stale view alive for a full TTL.

## Decision

Each mutation runs its side-effect-plus-bookkeeping sequence inside a `tokio::spawn` task that the handler merely awaits. Once the sequence starts it runs to completion whether or not the request survives. The store's internals live behind one `Arc<StoreInner>` so the spawned closures own what they touch.

The cache slot carries a monotonically increasing generation. Every store and every in-place edit bumps it. A coalesced build records the generation when it registers and its store is compare-and-store: when the generation moved during the walk, the build's result is served to its awaiters but not persisted. Newest write wins, explicitly, instead of by lock order.

## Consequences

A dropped request costs at most one completed unit of work, never a half-applied one, and a completed walk always lands in the cache even when its first requester vanished. A stale build can no longer clobber a newer mark or undo. The trade-off is one wasted walk when an edit lands mid-build, which is rare and bounded. Serializing `store_fresh` under the inflight lock was rejected during triage: a build that started before a mark still overwrites the slot with pre-mark data after the mark completes, so the lock cannot express the ordering the generation does. ADR-0002 and ADR-0027 carry amendment notes pointing here.
```

- [ ] **Step 4: Check and commit**

Run `mise run check` (the pre-commit hook rebuilds docs, and the Markdown hook expects unwrapped prose), then:

```bash
git add src/state.rs docs/adr/0002-marker-writes-edit-cache-in-place.md docs/adr/0027-substrate-consolidated-behind-rawviewstore.md docs/adr/0036-store-mutations-spawned-and-generation-guarded.md
git commit -m "docs(adr): record spawned mutations and the generation-guarded slot

ADR-0002 and ADR-0027 promised a lock-serialized store the code no
longer provides that way. Prose only: comments and ADRs, no behavior
change."
```

This closes cluster 1+2's doc reconciliation. No separate ledger entry; note the hash on F4's entry alongside the Task 2 hash.

---

### Task 6: Cap concurrent requests at 16 (F13, and F2's load-shedding half)

One `tower::limit::GlobalConcurrencyLimitLayer` on `router()`. Global, not per-route: `axum::Router::layer` wraps each route individually, so the plain `ConcurrencyLimitLayer` would mint a separate 16-permit semaphore per route. `GlobalConcurrencyLimitLayer` shares one semaphore across every route, which is what the spec's "cap of 16 on router()" means. Excess requests queue rather than erroring.

**Files:**
- Modify: `Cargo.toml`, `src/web.rs`

**Interfaces:**
- Consumes: `crate::state::BuildGate` and `RawViewStore::set_build_gate` from Task 2 (the router test needs to park handlers deterministically).
- Produces: `pub(crate) const MAX_IN_FLIGHT_REQUESTS: usize = 16;` in `src/web.rs`; `pub fn router(state: Arc<AppState>) -> Router` unchanged in signature; private `fn router_with_limit(state: Arc<AppState>, semaphore: Arc<tokio::sync::Semaphore>) -> Router`.

- [ ] **Step 1: Move tower into the runtime dependencies**

In `Cargo.toml`, add to `[dependencies]` (tower is already in the tree via axum, so this is a feature flag, not a new crate):

```toml
tower = { version = "0.5.3", default-features = false, features = ["limit"] }
```

Change the `[dev-dependencies]` entry to only add the test-side feature:

```toml
tower = { version = "0.5.3", default-features = false, features = ["util"] }
```

Cargo unions the features for test builds, so `tower::ServiceExt` keeps working in tests while the production binary carries only `limit`.

- [ ] **Step 2: Write the failing test**

In `src/web.rs::tests`. The test builds the router around a semaphore it can inspect, parks `MAX_IN_FLIGHT_REQUESTS` requests on a gated cold build, and observes that a cheap static-asset request queues behind the cap instead of being served:

```rust
#[tokio::test]
async fn the_concurrency_cap_queues_the_seventeenth_request() {
    use crate::state::BuildGate;

    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("Book/01.mp3"));
    let cfg = Config {
        library_roots: vec![dir.path().to_path_buf()],
        ttl_seconds: 60,
        ..Default::default()
    };
    let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
    let state = Arc::new(AppState::new(cfg, settings));
    let gate = BuildGate::new();
    state.store.set_build_gate(gate.clone());
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT_REQUESTS));
    let app = router_with_limit(Arc::clone(&state), Arc::clone(&semaphore));

    // Fill the cap: every page request joins the one gated cold build
    let mut parked = Vec::new();
    for _ in 0..MAX_IN_FLIGHT_REQUESTS {
        let app = app.clone();
        parked.push(tokio::spawn(async move {
            app.oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap()
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while semaphore.available_permits() > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the parked requests never consumed every permit");

    // With the cap full, even a cheap static asset queues
    let queued = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        app.clone().oneshot(
            Request::builder()
                .uri("/static/app.css")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await;
    assert!(queued.is_err(), "request 17 must wait behind the cap, not be served");

    // Releasing the build drains the queue; nothing errored
    gate.release.add_permits(1);
    for handle in parked {
        let response = handle.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "queued requests complete, never error");
    }
    let after = app
        .oneshot(
            Request::builder()
                .uri("/static/app.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::OK);
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo nextest run the_concurrency_cap_queues`
Expected: compile error (`MAX_IN_FLIGHT_REQUESTS` and `router_with_limit` do not exist). After Step 4's names exist but with no layer mounted, the test fails at `queued.is_err()`: the static asset is served immediately because nothing caps the router.

- [ ] **Step 4: Mount the layer**

In `src/web.rs`:

```rust
/// Cap on concurrently served requests. Each page render buffers the whole
/// library into one String (ADR-0032), so the cap bounds peak memory and
/// walk pile-up. Excess requests queue rather than erroring (ADR-0037)
pub(crate) const MAX_IN_FLIGHT_REQUESTS: usize = 16;

/// Build the application router with the shared state attached.
pub fn router(state: Arc<AppState>) -> Router {
    router_with_limit(
        state,
        Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
    )
}

/// Router around an injected semaphore. One shared semaphore caps every
/// route; Router::layer wraps each route individually, so the per-layer
/// ConcurrencyLimitLayer would mint a semaphore per route instead
fn router_with_limit(state: Arc<AppState>, semaphore: Arc<tokio::sync::Semaphore>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/mark", post(mark))
        .route("/unmark", post(unmark))
        .route("/rescan", post(rescan))
        .route("/refresh", get(refresh))
        .route("/static/htmx.min.js", get(assets::htmx_script))
        .route("/static/app.css", get(assets::app_css))
        .route("/static/app.js", get(assets::app_js))
        .layer(tower::limit::GlobalConcurrencyLimitLayer::with_semaphore(
            semaphore,
        ))
        .with_state(state)
}
```

- [ ] **Step 5: Run the new test and the full suite**

Run: `cargo nextest run the_concurrency_cap_queues`
Expected: PASS.

Run: `mise run test`
Expected: PASS (`mise run lint` also verifies the unused-dependency check accepts the tower move).

- [ ] **Step 6: Check and commit**

Run `mise run check`, then:

```bash
git add Cargo.toml Cargo.lock src/web.rs
git commit -m "fix(web): cap concurrent requests at 16

router() mounted no limiting layer, so nothing bounded concurrent page
renders (each buffers the whole library into one String) or the walk
pile-up behind unauthenticated POSTs (F13, amplifying F2). One shared
GlobalConcurrencyLimitLayer semaphore now covers every route; request
17 queues rather than erroring. tower moves from dev-dependencies to
dependencies with only the limit feature; it was already in the tree
via axum."
```

Annotate F13 (and note the shared-layer half on F2) in the ledger.

---

### Task 7: Rescan cooldown (F2)

The store records the last honored rescan. A `POST /rescan` inside a 5-second window skips the index clear and joins the in-flight or fresh build via `build_coalesced(true)`, returning normally. A double-click or a request loop costs one walk.

**Files:**
- Modify: `src/state.rs`
- Create: `docs/adr/0037-request-cap-and-rescan-cooldown.md`

**Interfaces:**
- Consumes: Task 4's spawned `rescan`, `StoreInner::build_coalesced`.
- Produces: `pub(crate) const RESCAN_COOLDOWN: Duration = Duration::from_secs(5);` in `src/state.rs`; `StoreInner` gains `last_rescan: std::sync::Mutex<Option<Instant>>` (constructed as `std::sync::Mutex::new(None)`).

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn a_rescan_inside_the_cooldown_keeps_the_index_and_the_slot() {
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
    // The first rescan is honored and warms the slot
    let _ = store.rescan().await;

    // A synthetic entry no real walk could produce: it survives only if
    // the second rescan skips the index clear
    let synthetic_path = std::path::PathBuf::from("/nonexistent/synthetic/marker/path");
    store.inner.dir_indices[0].insert(
        synthetic_path.clone(),
        scanner::CachedDir {
            mtime: std::time::UNIX_EPOCH,
            subdirs: std::sync::Arc::from(Vec::<std::path::PathBuf>::new()),
            cover_files: std::sync::Arc::from(Vec::<String>::new()),
            audio_files: std::sync::Arc::from(Vec::<String>::new()),
        },
    );
    let before = store.rebuild_count();

    let _ = store.rescan().await; // lands inside the 5-second window

    assert!(
        store.inner.dir_indices[0]
            .get_cloned(&synthetic_path)
            .is_some(),
        "a cooled-down rescan must not clear the index"
    );
    assert_eq!(
        store.rebuild_count(),
        before,
        "a cooled-down rescan joins the fresh slot instead of walking"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run a_rescan_inside_the_cooldown`
Expected: FAIL on both asserts: today every rescan clears the indices (the synthetic entry vanishes) and forces a build (the count bumps).

- [ ] **Step 3: Implement the cooldown**

Add the constant next to the other module-level items in `src/state.rs`:

```rust
/// Minimum spacing between honored rescans. A second click or a request
/// loop inside the window skips the index clear and joins the in-flight
/// or fresh build instead (silent coalescing, no error UI; ADR-0037)
pub(crate) const RESCAN_COOLDOWN: Duration = Duration::from_secs(5);
```

Add `last_rescan: std::sync::Mutex<Option<Instant>>` to `StoreInner` (a std mutex: it is held for nanoseconds and never across an await). Rework `rescan`:

```rust
pub(crate) async fn rescan(&self) -> Arc<RawView> {
    let honored = {
        let mut last = self
            .inner
            .last_rescan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let due = last.is_none_or(|at| at.elapsed() >= RESCAN_COOLDOWN);
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    if !honored {
        // Inside the window: keep the warm index, serve the fresh slot or
        // join the build already running
        return StoreInner::build_coalesced(&self.inner, true).await;
    }
    let inner = Arc::clone(&self.inner);
    tokio::spawn(async move {
        for index in &inner.dir_indices {
            index.clear();
        }
        StoreInner::build_coalesced(&inner, false).await
    })
    .await
    .expect("rescan task panicked")
}
```

Update the method's doc comment: forced cold scan, at most once per `RESCAN_COOLDOWN`, cooled-down calls coalesce.

- [ ] **Step 4: Run the new test and the full suite**

Run: `cargo nextest run a_rescan_inside_the_cooldown`
Expected: PASS.

Run: `mise run test`
Expected: PASS. Every existing rescan test issues a single rescan per fresh store, so the honored path stays pinned by `store_rescan_clears_the_dir_index_then_repopulates_it` and `store_rescan_refreshes_even_within_a_live_ttl`.

- [ ] **Step 5: Write ADR-0037**

Create `docs/adr/0037-request-cap-and-rescan-cooldown.md` (each paragraph one unwrapped line):

```markdown
# Requests cap at 16 in flight and rescans cool down for 5 seconds

Date: 2026-07-30.

## Context

`router()` mounted no limiting layer, so nothing bounded concurrent page renders (each buffers the whole library into one String, ADR-0032), and an unauthenticated `POST /rescan` loop discarded the mtime index and forced an uncapped cold walk per request, a durable denial of service on the network mounts this tool targets.

## Decision

A `tower::limit::GlobalConcurrencyLimitLayer` caps the router at 16 concurrently served requests through one shared semaphore. Excess requests queue rather than erroring. The store records the instant of the last honored rescan; a rescan landing within 5 seconds skips the index clear and joins the in-flight or fresh build via `build_coalesced`, returning normally. Silent coalescing, no error UI: a double-click or a request loop costs one walk.

A request timeout layer was considered and rejected. A timeout is the only guardrail that can kill a legitimate cold-scan page load on a slow network mount, and the cap plus the cooldown cover the audited abuse cases. A generous timeout can be added later if a hang is ever observed.

## Consequences

A rescan loop no longer keeps the server permanently cold-walking, and slow readers cannot pile up unbounded page buffers. The 17th concurrent request waits instead of failing, the honest behavior for a self-hosted tool. Buffered rendering itself stays as ADR-0032 chose it.
```

- [ ] **Step 6: Check and commit**

Run `mise run check`, then:

```bash
git add src/state.rs docs/adr/0037-request-cap-and-rescan-cooldown.md
git commit -m "fix(state): honor rescans at most once per five seconds

Each POST /rescan discarded the warm mtime index and forced an uncapped
cold walk, so a trivial request loop kept the server permanently
cold-walking the library (F2). A rescan inside the 5-second window now
skips the index clear and joins the in-flight or fresh build, returning
normally. ADR-0037 records the cooldown and the request cap together."
```

Annotate F2 in the ledger.

---

### Task 8: Count skipped directories and cap the walk depth (F6 scanner half, F7)

The walk's existing skip-and-warn branches increment a per-root skipped-directory count carried on `RootScan::Walked`. The depth cap feeds the same counter, so depth capping surfaces through the same warning instead of becoming a second silent hole.

**Files:**
- Modify: `src/scanner.rs`, `src/synthetic.rs` (one constructor), `src/tree.rs` (one exhaustive match, one test helper)

**Interfaces:**
- Consumes: nothing from earlier tasks (independent of the store rework).
- Produces: `WalkStats` gains `pub dirs_skipped: usize`; `RootScan::Walked` gains `skipped_dirs: usize`; `ScanSettings` gains `pub(crate) const MAX_DEPTH: usize = 64;`. Task 9 reads `skipped_dirs` off `RootScan::Walked`.

- [ ] **Step 1: Write the failing tests**

In `src/scanner.rs::tests`:

```rust
#[test]
#[cfg(unix)]
fn an_unreadable_subdirectory_is_counted_and_siblings_still_scan() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("Locked/Book/01.mp3"));
    touch(&dir.path().join("Open/01.mp3"));
    let locked = dir.path().join("Locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root (or CAP_DAC_OVERRIDE) reads through the chmod; nothing to observe
    if std::fs::read_dir(&locked).is_ok() {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let settings = default_settings(&[]);
    let (folders, stats) = scan_warm(dir.path(), &settings, &DirIndex::new());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(stats.dirs_skipped, 1, "the unreadable directory is counted");
    assert!(
        folders.iter().any(|f| f.rel_path.as_os_str() == "Open"),
        "the readable sibling still scans"
    );
    assert!(
        !folders
            .iter()
            .any(|f| f.rel_path.to_string_lossy().starts_with("Locked/")),
        "nothing below the unreadable directory is listed"
    );
}

#[test]
fn walk_depth_caps_at_max_depth_and_counts_the_skipped_subtree() {
    let dir = tempfile::tempdir().unwrap();
    let mut deep = dir.path().to_path_buf();
    // d0 sits at depth 1, so d64 sits at depth 65, one past the cap
    for i in 0..=ScanSettings::MAX_DEPTH {
        deep.push(format!("d{i}"));
    }
    touch(&deep.join("01.mp3"));
    let settings = default_settings(&[]);
    let (folders, stats) = scan_warm(dir.path(), &settings, &DirIndex::new());

    assert_eq!(stats.dirs_skipped, 1, "the capped subtree root is counted");
    assert!(
        folders
            .iter()
            .all(|f| f.rel_path.components().count() <= ScanSettings::MAX_DEPTH),
        "no folder below the cap is listed"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run an_unreadable_subdirectory walk_depth_caps`
Expected: compile error (`dirs_skipped` and `MAX_DEPTH` do not exist).

- [ ] **Step 3: Implement counting and the cap**

In `src/scanner.rs`:

1. `WalkStats` gains a field:

```rust
    /// Directories the walk could not read, plus subtree roots skipped by
    /// the depth cap. Nonzero renders the per-root partial-scan warning
    pub dirs_skipped: usize,
```

2. `list_dir_all`'s `read_dir` error branch returns `stats: WalkStats { dirs_skipped: 1, ..WalkStats::default() }` instead of `WalkStats::default()`. The per-entry error branch (the `Err(err) => { warn; continue; }` arm) adds `stats.dirs_skipped += 1;` before its `continue`. The stat-failure branch in `read_dir_all` stays uncounted: it falls to an uncached listing, and when the directory is genuinely unreadable that listing's `read_dir` fails and counts there, so counting the stat too would tally the same directory twice. The observable contract, one increment per unreadable directory, holds.

3. The constant, on `impl ScanSettings`:

```rust
    /// Deepest level the walk descends, counted from the root at zero.
    /// Subtrees below the cap are skipped into `dirs_skipped`, bounding the
    /// per-level render recursion the tree otherwise feeds unchecked.
    // ponytail: fixed ceiling. 64 dwarfs any real audiobook layout; lift it
    // into config if a legitimate library ever trips the warning
    pub(crate) const MAX_DEPTH: usize = 64;
```

4. `scan_warm` tracks the level number (it is already level-synchronous) and stops descending at the cap:

```rust
    let mut depth = 0usize;
    while !frontier.is_empty() {
        // ... the existing parallel level read ...
        let mut next = Vec::new();
        for mut dir in level {
            stats.dirs_visited += dir.stats.dirs_visited;
            stats.entries_seen += dir.stats.entries_seen;
            stats.dirs_reused += dir.stats.dirs_reused;
            stats.dirs_skipped += dir.stats.dirs_skipped;
            if let Some(cached) = dir.cache_update.take() {
                index.insert(dir.path.clone(), cached);
            }
            if let Some(folder) = dir.folder.take() {
                out.push(folder);
            }
            if depth >= ScanSettings::MAX_DEPTH {
                if !dir.children.is_empty() {
                    stats.dirs_skipped += dir.children.len();
                    tracing::warn!(
                        dir = %dir.path.display(),
                        depth,
                        skipped = dir.children.len(),
                        "walk depth cap reached; skipping deeper subtrees"
                    );
                }
            } else {
                for child in dir.children.iter() {
                    next.push((child.clone(), dir.child_covered));
                }
            }
        }
        frontier = next;
        depth += 1;
    }
```

5. `RootScan::Walked` gains the field and `scan_root` fills it:

```rust
    Walked {
        /// The canonicalized root path the walk ran against.
        canonical_path: PathBuf,
        /// Every folder the walk produced. Empty when no entry qualified.
        folders: Vec<ScannedFolder>,
        /// Directories the walk skipped: unreadable, or past the depth cap.
        skipped_dirs: usize,
    },
```

In `scan_root`: `RootScan::Walked { canonical_path: canonical, folders, skipped_dirs: stats.dirs_skipped }`.

6. Ripple the new field through the compiler errors:

- `src/synthetic.rs:77`: the wrapper adds `skipped_dirs: 0`.
- `src/tree.rs:126`: `tree::build`'s match arm becomes `RootScan::Walked { canonical_path, folders, .. }`.
- `src/tree.rs:286` (the test helper constructing a `Walked`): adds `skipped_dirs: 0`.
- Any other construction or exhaustive-destructure site the compiler flags gets `skipped_dirs: 0` or `..` respectively (`src/state.rs`, `src/raw_view.rs`, and `src/demo/` all already destructure with `..`).

- [ ] **Step 4: Run the new tests and the full suite**

Run: `cargo nextest run an_unreadable_subdirectory walk_depth_caps`
Expected: PASS.

Run: `mise run test`
Expected: PASS.

- [ ] **Step 5: Check and commit**

Run `mise run check`, then two commits:

```bash
git add src/scanner.rs src/synthetic.rs src/tree.rs
git commit -m "fix(scanner): count directories the walk skips

An unreadable subtree silently vanished from results: the walk logged a
warning and dropped it, the root rendered as successfully walked, and
every gap inside it was absent from the tree and the coverage math, so
the tool reported a cleaner library than exists (F6). The skip-and-warn
branches now increment a per-root skipped_dirs count carried on
RootScan::Walked; the render half surfaces it next."
git add src/scanner.rs
git commit -m "fix(scanner): cap the walk depth at 64

Rendering recurses per tree level with no bound, so a pathological
directory chain aborts the process on stack overflow, with a quadratic
render blowup well before that (F7). The walk now stops descending at
MAX_DEPTH = 64 and counts each capped subtree into skipped_dirs, so the
cap surfaces through the partial-scan warning instead of becoming a
second silent hole."
```

(If the two changes are hard to stage separately, land them as one commit naming both findings.) Annotate F6 (scanner half) and F7 in the ledger.

---

### Task 9: Render the partial-scan warning strip (F6 render half)

When a root's `skipped_dirs` is nonzero, its section renders a warning strip in the existing per-root banner slot. Count only; the skipped paths stay in the server log. Coverage math is unchanged.

**Files:**
- Modify: `src/web/render.rs`, `assets/app.css`, `src/web.rs` (test only)

**Interfaces:**
- Consumes: `RootScan::Walked.skipped_dirs` from Task 8.
- Produces: `RootSection` gains `skipped_dirs: usize`; rendered markup `div.alert.alert-warning` inside `details.root-fold`, before the state match.

- [ ] **Step 1: Write the failing render tests**

In `src/web/render.rs::tests`:

```rust
#[test]
fn a_root_with_skipped_directories_renders_the_partial_scan_warning() {
    let mut view = section(
        "/lib",
        forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
        1,
    );
    view.skipped_dirs = 3;
    let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
    assert!(html.contains("alert-warning"));
    assert!(html.contains("3 folders couldn't be read; results for this root may be incomplete."));
    assert!(html.contains("Book"), "the readable rows still render");
}

#[test]
fn a_fully_read_root_renders_no_partial_scan_warning() {
    let view = section(
        "/lib",
        forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
        1,
    );
    let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
    assert!(!html.contains("alert-warning"));
}

#[test]
fn one_skipped_directory_reads_in_the_singular() {
    let mut view = section("/lib", RootState::Clean, 1);
    view.skipped_dirs = 1;
    let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
    assert!(html.contains("1 folder couldn't be read; results for this root may be incomplete."));
}
```

Note on the copy assertions: maud escapes `&`, `<`, `>`, and `"` but not the apostrophe, so `couldn't` survives verbatim. If an assertion fails on the apostrophe, check the escaping and split the assertion around it rather than changing the copy.

And the end-to-end web test in `src/web.rs::tests`:

```rust
#[tokio::test]
#[cfg(unix)]
async fn an_unreadable_subdirectory_renders_the_partial_scan_warning() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("Locked/Book/01.mp3"));
    touch(&dir.path().join("Open/01.mp3"));
    let locked = dir.path().join("Locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let response = app_for(dir.path())
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("alert-warning"));
    assert!(body.contains("1 folder couldn't be read; results for this root may be incomplete."));
    assert!(body.contains("Open"), "the readable sibling still renders");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run partial_scan_warning one_skipped_directory`
Expected: compile error (`RootSection` has no `skipped_dirs`).

- [ ] **Step 3: Implement the strip**

1. `RootSection` gains the field (docs in the existing style):

```rust
    /// Directories the walk skipped for this root: unreadable, or past the
    /// depth cap. Nonzero renders the partial-scan warning strip.
    skipped_dirs: usize,
```

2. `package_section` fills it:

```rust
    skipped_dirs: match scan {
        RootScan::Walked { skipped_dirs, .. } => *skipped_dirs,
        RootScan::Failed { .. } => 0,
    },
```

3. In `render_section`, directly after the `@if let Some(message) = error` alert (the existing per-root banner slot):

```rust
    @if section.skipped_dirs > 0 {
        div.alert.alert-warning {
            (PreEscaped(include_str!("../../assets/svg/warning.svg")))
            span {
                (section.skipped_dirs) " " (folder_word(section.skipped_dirs))
                " couldn't be read; results for this root may be incomplete."
            }
        }
    }
```

4. A pluralization helper next to `gap_word`:

```rust
/// Pluralize "folder" for the partial-scan warning strip.
fn folder_word(n: usize) -> &'static str {
    if n == 1 { "folder" } else { "folders" }
}
```

5. Update the test helper `section(..)` in `render.rs::tests` to construct `skipped_dirs: 0`.

6. In `assets/app.css`, after `.alert-error`, mirroring its shape with the warning tokens (the same pair `.badge-warning` already uses):

```css
.alert-warning {
  background: color-mix(in srgb, var(--color-warning) 14%, var(--color-base-100));
  border: var(--border) solid color-mix(in srgb, var(--color-warning) 35%, transparent);
  color: var(--color-base-content);
}
.alert-warning .icon {
  color: var(--color-warning-text);
}
```

The `.icon` override matters: the generic `.alert .icon` rule hardcodes `--color-error-text`. Verify `assets/svg/warning.svg` carries `class="icon"` like `error.svg`; if not, add it there rather than widening the selector.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run partial_scan_warning one_skipped_directory`
Expected: PASS.

Run: `mise run test`
Expected: PASS. The asset change also puts `mise run test:accent` in the pre-commit path; run it now to catch contrast complaints early (the warning strip must hold up in both themes; measure the rendered element, not the tokens).

- [ ] **Step 5: Verify visually in the explore harness**

Per CLAUDE.md: check `lsof -i :13379` for a squatter, then run `cargo run --example explore -- messy-shelf` (bump `--port` if taken). Read the seeded root path off the rendered section heading in the browser, `chmod 000` one of its subdirectories, click Rescan, and confirm the warning strip appears with the right count while the readable siblings keep rendering, in both light and dark themes. Hand the user the clickable localhost link and point them at the strip. Restore the permissions and stop the harness (match by cwd, never blanket-kill) when they are done.

- [ ] **Step 6: Check and commit**

Run `mise run check`, then:

```bash
git add src/web/render.rs src/web.rs assets/app.css
git commit -m "fix(web): warn when a root's scan skipped directories

A root with an unreadable subtree rendered as successfully walked, so
the numbers read as truth when they are a floor (F6). A nonzero
skipped_dirs now renders a count-only warning strip in the per-root
banner slot; paths stay in the server log, and coverage math is
unchanged."
```

Annotate F6 in the ledger.

---

### Task 10: Trivial fixes (F14, F15 mislabel, F16, F17)

Four independent one-site fixes, one commit each.

**Files:**
- Modify: `CONTEXT.md`, `src/web/render.rs` (test only), `src/state.rs`, `Cargo.toml`, `src/shutdown.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: nothing later tasks rely on, except the `write_marker` error mapping Task 11's pins exercise.

- [ ] **Step 1 (F14): Amend CONTEXT.md and pin the floored digit**

In `CONTEXT.md` line 72, replace the final sentence of the Library coverage entry:

Old: ``The reported percentage is `Math.round(covered / total * 100)`.``
New: ``The reported percentage is `Math.floor(covered / total * 100)`, floored so a single remaining gap never reads as 100%.``

Then the pin in `src/web/render.rs::tests` (a 199-of-200 fixture: one flagged leaf, total 200):

```rust
#[test]
fn coverage_percentage_floors_on_a_199_of_200_fixture() {
    let view = vec![section(
        "/lib",
        forest(vec![flagged_leaf("Gap", "Gap", &["01.mp3"])]),
        200,
    )];
    let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
    // 199 covered of 200 floors to 99, never rounds to a false 100
    assert!(html.contains(r#"id="coverage-pct">99<"#));
    assert!(html.contains(r#"aria-valuenow="199""#));
    assert!(html.contains(r#"aria-valuemax="200""#));
}
```

Before landing, confirm the exact substring against `gap_summary`/`coverage_bar`'s markup (the pinned fact is the digit 99; adjust the tag shape around it if the element nests differently). Run `cargo nextest run coverage_percentage_floors` (PASS expected first try: this is an expected-pass pin, the code already floors), then commit in two pieces:

```bash
git add CONTEXT.md
git commit -m "docs(context): coverage percentage floors rather than rounds"
git add src/web/render.rs
git commit -m "test(web): pin the floored coverage digit on a 199-of-200 fixture

CONTEXT.md promised Math.round while both implementations deliberately
floor so a single remaining gap never reads 100% (F14). The contract
now states the floor and this pin holds the digit at 99."
```

Annotate F14 in the ledger.

- [ ] **Step 2 (F15 mislabel): Preserve the error kind in `write_marker`'s canonicalize**

First the failing test, in `src/state.rs::tests`:

```rust
#[test]
#[cfg(unix)]
fn write_marker_reports_a_permission_failure_not_a_missing_target() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Sealed/Book")).unwrap();
    let sealed = dir.path().join("Sealed");
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root traverses through the chmod; nothing to observe then
    if std::fs::metadata(sealed.join("Book")).is_ok() {
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let err = write_marker(dir.path(), "Sealed/Book", Marker::NoEbook);
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
    match err.unwrap_err() {
        WriteError::WriteFailed(e) => {
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied)
        }
        other => panic!("an EACCES must not report as {other:?}"),
    }
}
```

Run: `cargo nextest run write_marker_reports_a_permission_failure`
Expected: FAIL with `TargetMissing` (the `map_err(|_| WriteError::TargetMissing)` discards the kind).

Then the fix in `src/state.rs`: replace both `canonicalize(...).map_err(|_| WriteError::TargetMissing)` sites in `write_marker` with a shared helper (leave `delete_marker` alone: its NotFound-is-success arms are a different, deliberate contract):

```rust
/// Canonicalize a mark target, keeping the error kind honest: only a
/// missing path is TargetMissing, anything else (EACCES and friends)
/// surfaces as the write failure it is
fn canonicalize_mark_target(path: &Path) -> Result<PathBuf, WriteError> {
    std::fs::canonicalize(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => WriteError::TargetMissing,
        _ => WriteError::WriteFailed(e),
    })
}
```

`write_marker` becomes:

```rust
    let canonical_root = canonicalize_mark_target(root)?;
    let target = canonical_root.join(rel);
    let canonical_target = canonicalize_mark_target(&target)?;
```

Run the test (PASS) plus `cargo nextest run write_marker` (the existing `write_marker_missing_target_is_an_error` still passes: a missing path is NotFound). Commit:

```bash
git add src/state.rs
git commit -m "fix(state): report a mark permission failure as a write failure

write_marker mapped every canonicalize error to TargetMissing, so an
EACCES on the target reported \"target folder does not exist\" instead
of a permission failure (F15). Only NotFound maps to TargetMissing now;
every other kind surfaces as WriteFailed with the real error."
```

Annotate F15's mislabel half in the ledger.

- [ ] **Step 3 (F16): Enable `clippy::undocumented_unsafe_blocks`**

In `Cargo.toml`'s `[lints.clippy]` table, after the pedantic allow-list:

```toml
undocumented_unsafe_blocks = "deny" # Restriction-group pick: every unsafe block carries a SAFETY comment. The one existing block (shutdown test) already complies.
```

Run: `mise run lint`
Expected: PASS (the single `#[cfg(test)]` unsafe block at `src/shutdown.rs:54` already carries its `// SAFETY:` comment). Commit:

```bash
git add Cargo.toml
git commit -m "chore(lints): deny undocumented unsafe blocks

The habit already holds (the crate's one unsafe block carries its
SAFETY comment) but nothing enforced it (F16)."
```

Annotate F16 in the ledger.

- [ ] **Step 4 (F17): Match the `ctrl_c()` results in shutdown**

`tokio::signal::ctrl_c()` returns `Err` when the handler cannot be installed, not when a signal arrives. All three sites discard it, so an install failure resolves the shutdown future immediately and stops the server at startup while logging "received SIGINT". No new test: forcing a handler-install failure needs an environment seam (seccomp, fd exhaustion), the same machinery-for-its-own-sake trade the F23 wontfix records, and the existing `sigterm_resolves_signal_future` test pins the good path. Replace the body of `signal()` in `src/shutdown.rs`:

```rust
pub async fn signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};
        let mut term = match unix_signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "could not install SIGTERM handler, waiting for SIGINT only"
                );
                match tokio::signal::ctrl_c().await {
                    Ok(()) => tracing::info!("received SIGINT, shutting down"),
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "could not install the SIGINT handler either; signal-driven shutdown is disabled"
                        );
                        std::future::pending::<()>().await;
                    }
                }
                return;
            }
        };
        tokio::select! {
            sigint = tokio::signal::ctrl_c() => match sigint {
                Ok(()) => tracing::info!("received SIGINT, shutting down"),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "could not install SIGINT handler, waiting for SIGTERM only"
                    );
                    term.recv().await;
                    tracing::info!("received SIGTERM, shutting down");
                }
            },
            _ = term.recv() => tracing::info!("received SIGTERM, shutting down"),
        }
    }
    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => tracing::info!("received Ctrl-C, shutting down"),
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "could not install the Ctrl-C handler; signal-driven shutdown is disabled"
                );
                std::future::pending::<()>().await;
            }
        }
    }
}
```

Extend the function's doc comment second paragraph to cover all three fallbacks: when a handler cannot be installed the future keeps waiting on whatever remains, and when nothing remains it stays pending forever rather than stopping the server.

Run: `cargo nextest run sigterm_resolves`
Expected: PASS. Commit:

```bash
git add src/shutdown.rs
git commit -m "fix(shutdown): stay pending when a signal handler fails to install

ctrl_c() errors when the handler cannot be installed, not when a signal
arrives. All three sites discarded the Result, so an install failure
resolved the shutdown future at startup and logged a SIGINT that never
happened, contradicting the function's own doc contract (F17). Each
site now matches the Result and keeps waiting on the other signal, or
pends forever when none remains."
```

Annotate F17 in the ledger, then run `mise run check` once over the task's combined state.

---

### Task 11: Pin batch, server side (F15 arms, F19, F20, F21, F8)

Expected-pass pinning tests following existing repo patterns. Write each, run it, watch it pass, commit. If one fails, stop: that is a real defect, reassess against the ledger before touching code.

**Files:**
- Test: `src/state.rs`, `src/scanner.rs`, `src/config.rs`, `src/web.rs`

**Interfaces:**
- Consumes: Task 10's `canonicalize_mark_target` behavior, Task 8's scanner shape.
- Produces: tests only.

- [ ] **Step 1 (F15): Pin the `WriteFailed` and undo-failure arms**

In `src/state.rs::tests`:

```rust
#[test]
#[cfg(unix)]
fn write_marker_readonly_dir_reports_write_failed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Book")).unwrap();
    let book = dir.path().join("Book");
    std::fs::set_permissions(&book, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root writes through the chmod; nothing to observe then
    if std::fs::write(book.join("probe"), b"").is_ok() {
        std::fs::set_permissions(&book, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let err = write_marker(dir.path(), "Book", Marker::NoEbook);
    std::fs::set_permissions(&book, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(err.unwrap_err(), WriteError::WriteFailed(_)));
}

#[test]
#[cfg(unix)]
fn delete_marker_readonly_dir_reports_write_failed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Book")).unwrap();
    let book = dir.path().join("Book");
    std::fs::write(book.join(".no_ebook"), b"").unwrap();
    std::fs::set_permissions(&book, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(book.join("probe"), b"").is_ok() {
        std::fs::set_permissions(&book, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let err = delete_marker(dir.path(), "Book", Marker::NoEbook);
    std::fs::set_permissions(&book, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(err.unwrap_err(), WriteError::WriteFailed(_)));
}

#[tokio::test]
async fn store_remove_mark_failure_returns_current_raw_view() {
    // The undo mirror of store_write_mark_failure_returns_current_raw_view
    let dir = tempfile::tempdir().unwrap();
    crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
    let _warm = store.current().await;
    let rebuilds_before = store.rebuild_count();

    let err = store
        .remove_mark(0, "..", Marker::NoEbook)
        .await
        .unwrap_err();
    let raw = match err {
        WriteFailure::Failed {
            error: WriteError::OutsideRoots,
            raw,
        } => raw,
        other => panic!("expected Failed with OutsideRoots, got {other:?}"),
    };
    assert_eq!(
        store.rebuild_count(),
        rebuilds_before,
        "warm undo failure must not rebuild",
    );
    assert_eq!(raw.len(), 1, "raw carries one section per library root");
}
```

Run: `cargo nextest run readonly_dir_reports store_remove_mark_failure`
Expected: PASS. Commit `test(state): pin the write and undo io-failure arms` with a body noting these close F15's unpinned arms. Annotate F15 in the ledger.

- [ ] **Step 2 (F19): Pin the backwards-mtime re-list**

In `src/scanner.rs::tests`, next to `warm_scan_relists_a_changed_dir_and_flips_the_gap`:

```rust
#[test]
fn warm_scan_relists_a_dir_whose_mtime_moved_backwards() {
    // The index compares mtime by equality, not newer-than, so a clock
    // step or restored backup still re-lists. Pin the safe direction
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("Book");
    touch(&book.join("01.mp3"));
    let settings = default_settings(&[]);
    let index = DirIndex::new();
    let (first, _) = scan_warm(dir.path(), &settings, &index);
    assert!(first.iter().any(|f| f.missing_ebook), "the gap is indexed");

    touch(&book.join("Book.epub"));
    // Push the mtime backwards, before anything the index has seen
    std::fs::File::open(&book)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH)
        .unwrap();
    let (second, _) = scan_warm(dir.path(), &settings, &index);
    let book_folder = second
        .iter()
        .find(|f| f.rel_path.as_os_str() == "Book")
        .unwrap();
    assert!(
        !book_folder.missing_ebook,
        "a backwards mtime must re-list, not reuse the stale entry"
    );
}
```

Run it (PASS), commit `test(scanner): pin backwards-mtime re-listing`, annotate F19.

- [ ] **Step 3 (F20): Pin `ConfigError::Read`**

In `src/config.rs::tests`, next to `unknown_keys_are_rejected`:

```rust
#[test]
fn an_unreadable_config_file_is_a_read_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Config::from_file(&dir.path().join("absent.toml")),
        Err(ConfigError::Read { .. })
    ));
}
```

Run it (PASS), commit `test(config): pin the unreadable-config Read arm`, annotate F20.

- [ ] **Step 4 (F21): Pin the file-as-root banner path**

Scanner half, in `src/scanner.rs::tests`:

```rust
#[test]
fn a_file_root_fails_the_scan_as_not_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("root.txt");
    std::fs::write(&file, b"").unwrap();
    let scan = scan_root(&file, &default_settings(&[]), &DirIndex::new());
    let RootScan::Failed { message, .. } = scan else {
        panic!("expected Failed for a file root");
    };
    assert_eq!(message, "not a directory");
}
```

Web half, in `src/web.rs::tests` next to `a_failed_root_scan_renders_the_error_banner_not_a_500`:

```rust
#[tokio::test]
async fn a_file_root_renders_the_error_banner() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("root.txt");
    std::fs::write(&file, b"").unwrap();
    let response = app_for(&file)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains("Could not scan this root:"));
    assert!(body.contains("not a directory"));
}
```

Run both (PASS), commit `test(scanner): pin the file-root banner path` (one commit, both halves), annotate F21.

- [ ] **Step 5 (F8): Pin the accepted non-UTF-8 behavior**

In `src/state.rs::tests`:

```rust
#[tokio::test]
#[cfg(unix)]
async fn a_non_utf8_name_scans_lossily_and_its_mark_fails_as_missing() {
    // Accepted v1 behavior (F8 wontfix): the walk flags the folder, the
    // name renders with U+FFFD, and the lossy rel round-trips to a path
    // that does not exist, so the mark fails with the missing-target arm
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join(std::ffi::OsStr::from_bytes(b"Bo\xffok"));
    // APFS and friends reject non-UTF-8 names; nothing to pin there
    if std::fs::create_dir(&folder).is_err() {
        return;
    }
    crate::scenarios::touch(&folder.join("01.mp3"));
    let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

    let raw = store.current().await;
    let scanner::RootScan::Walked { folders, .. } = &raw[0] else {
        panic!("expected Walked");
    };
    let lossy = folders
        .iter()
        .find(|f| f.directly_holds_audio)
        .expect("the non-UTF-8 folder is flagged")
        .rel_path
        .to_string_lossy()
        .into_owned();
    assert!(
        lossy.contains('\u{FFFD}'),
        "the name reaches the tree lossily"
    );

    let err = store.write_mark(0, &lossy, Marker::NoEbook).await.unwrap_err();
    assert!(matches!(
        err,
        WriteFailure::Failed {
            error: WriteError::TargetMissing,
            ..
        }
    ));
}
```

Run it (PASS on Linux and CI; on macOS/APFS it exits at the `create_dir` guard), commit `test(state): pin lossy non-UTF-8 scanning and its dead mark`, annotate F8's pin. Run `mise run check` once over the task's combined state before the last commit.

---

### Task 12: Pin batch, client contracts (F18, F25, F26, F27)

Occurrence-count and substring pins. Same rule as Task 11: expected-pass, and a failure means stop and reassess.

**Files:**
- Test: `src/web/render.rs`, `src/web/assets.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: tests only.

- [ ] **Step 1 (F18): Pin container actionability by occurrence count**

A `contains` check can never isolate a container's buttons because an actionable container always sits above an actionable leaf, so count occurrences. In `src/web/render.rs::tests`:

```rust
#[test]
fn container_and_leaf_each_emit_their_own_actions_group() {
    let view = vec![section(
        "/lib",
        forest(vec![container(
            "Author",
            "Author",
            vec![flagged_leaf("Gap", "Author/Gap", &["01.mp3"])],
        )]),
        1,
    )];
    for mode in [ViewMode::GapsOnly, ViewMode::All] {
        let html = render_view(&view, &default_links(), mode, 0).into_string();
        assert_eq!(
            html.matches(r#"class="actions-group""#).count(),
            2,
            "container and leaf each carry one actions group in {} view",
            mode.as_query(),
        );
    }
}
```

Run it (PASS), commit `test(web): pin container actionability by occurrence count`, annotate F18.

- [ ] **Step 2 (F25): Fence the view toggle out of client storage**

In `src/web/assets.rs::tests`, in the style of `stylesheet_does_not_carry_the_removed_gap_session_class` (app.js uses double-quoted literals, prepaint.js single):

```rust
#[test]
fn scripts_do_not_persist_the_view_toggle() {
    // CONTEXT.md:31: the show-all toggle rides ?view= only and a reload
    // lands on gaps-only. Fence any localStorage view key out
    for shape in [
        r#"localStorage.getItem("view")"#,
        r#"localStorage.setItem("view""#,
    ] {
        assert!(!APP_JS_BYTES.contains(shape), "app.js must not persist {shape}");
    }
    for shape in [
        "localStorage.getItem('view')",
        "localStorage.setItem('view'",
    ] {
        assert!(
            !PREPAINT_JS_BYTES.contains(shape),
            "prepaint.js must not persist {shape}"
        );
    }
    // The view still rides the query string
    assert!(APP_JS_BYTES.contains("/refresh?view="));
}
```

Run it (PASS), commit `test(assets): fence the view toggle out of client storage`, annotate F25.

- [ ] **Step 3 (F26): Pin the toast cap and eviction**

```rust
#[test]
fn app_script_caps_the_toast_stack_and_evicts_the_oldest() {
    // CONTEXT.md:36: at most three toasts, the oldest evicted on overflow
    assert!(APP_JS_BYTES.contains("var MAX_TOASTS = 3;"));
    assert!(APP_JS_BYTES.contains("while (stack.children.length >= MAX_TOASTS)"));
    assert!(APP_JS_BYTES.contains("hardRemove"));
}
```

Run it (PASS), commit `test(assets): pin the toast cap and oldest-first eviction`, annotate F26.

- [ ] **Step 4 (F27): Pin the visibility-gated poll**

```rust
#[test]
fn app_script_gates_the_refresh_poll_on_tab_visibility() {
    // ADR-0034: a hidden tab skips its poll and fires a catch-up on
    // becoming visible. The last unpinned ADR clause from the sweep
    assert!(APP_JS_BYTES.contains(r#"if (document.visibilityState !== "visible") return;"#));
    assert!(APP_JS_BYTES.contains(r#"addEventListener("visibilitychange""#));
    assert!(APP_JS_BYTES.contains(r#"if (document.visibilityState === "visible") pollOnce();"#));
}
```

Run it (PASS), commit `test(assets): pin the visibility-gated refresh poll`, annotate F27.

- [ ] **Step 5: Final sweep**

Run `mise run check` one last time. Confirm every pursued finding in `.scratch/v1-stability/FINDINGS.md` now carries a `Landed in <hash>.` line: F1-F7, F10-F21, F25-F27, plus F8's pin. F28 stays a user action (merge PRs 4-7 from the GitHub web UI). Verify `git log --oneline` reads as granular conventional commits with no squash and no attribution trailers.
