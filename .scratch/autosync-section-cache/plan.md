# Autosync Section-Content Render Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Switch `autosync`'s per-broadcast dedup basis from rendered HTML to section content (audit #5). On no-change ticks the loop skips both the render and the push. The win is bounded by the per-section render cost measured in `benches/render.rs`; spec has the numbers.

**Architecture:** Pre-req: derive `Hash` on `scanner::RootScan` and `scanner::ScannedFolder`. Then introduce `section_content_hash(&RootScan) -> u64` in `src/autosync.rs`, hoist it above the per-mode loop in `compute_pushes`, rename `last_hash` to `last_content_hash`, and route `snapshot_and_seed`'s seed through the same helper. Delete `rendered_oob_with_hash` and `stable_hash`. Instrument `compute_pushes` render count under `#[cfg(test)]` to let tests assert render avoidance, mirroring the `rebuild_count` pattern in `state.rs` (commit `7e5254e`). Update ADR-0024.

**Tech Stack:** Rust, `cargo test`, existing `mise` pre-commit hook.

## Global Constraints

- Changes confined to `src/autosync.rs`, `src/scanner.rs`, and `docs/adr/0024-autosync-section-level-oob-swap.md`. No other file is touched.
- One public-API change: `Hash` derive on `RootScan` and `ScannedFolder`. Both types are simple data; the derive compiles from their existing fields.
- Existing autosync tests (`first_call_pushes_every_mode_root_pair`, `identical_second_call_pushes_nothing`, `changed_root_produces_pushes_only_for_that_root`, `mode_with_no_subscribers_is_skipped_and_its_hashes_stay_untouched`, `attach_sends_snapshot_first_and_registers_subscriber`, `second_subscribe_for_same_mode_does_not_overwrite_baseline_hashes`, the abort-and-respawn cases) must continue to pass without modification. They assert on user-visible behavior (which events arrive on which channels), not on the hash basis.
- Pre-commit hook runs fmt, clippy, and `cargo doc -D warnings`. Never bypass with `--no-verify`.
- Commits follow Conventional Commits. `chore(scope): ...` for the derive prep; `feat(autosync): ...` for the dedup-basis swap and the test instrumentation; `test(autosync): ...` for the new behavior tests; `docs(adr): ...` for the ADR update.

---

### Task 1: Derive `Hash` on `RootScan` and `ScannedFolder`

**Files:**
- Modify: `src/scanner.rs` (the `#[derive(...)]` lines on `RootScan` at L254 and `ScannedFolder` at L232)

**Interfaces:**
- Produces: `Hash` impls on the two types. Task 2 consumes them through a derived `DefaultHasher` walk inside `section_content_hash`.

- [ ] **Step 1: Add `Hash` to both derive lists**

Change the derive on `RootScan` from `#[derive(Debug, Clone)]` to `#[derive(Debug, Clone, Hash)]`.

Change the derive on `ScannedFolder` from `#[derive(Debug, Clone, PartialEq, Eq)]` to `#[derive(Debug, Clone, PartialEq, Eq, Hash)]`.

- [ ] **Step 2: Confirm the derive compiles**

Run: `cargo check -p missing-ebooks --all-targets`

Expected: clean. All fields (`PathBuf`, `bool`, `String`, `Vec<String>`, `Vec<ScannedFolder>`) implement `Hash`, so no field-level intervention is needed.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test -p missing-ebooks`

Expected: all tests pass. No behavior change.

- [ ] **Step 4: Commit**

```bash
git add src/scanner.rs
git commit -m "chore(scanner): derive Hash on RootScan and ScannedFolder

Pre-req for the autosync section-content render cache (audit #5).
Both types are simple data and all fields already implement Hash, so
the derive compiles without further changes. No behavior change."
```

---

### Task 2: Switch dedup basis from rendered HTML to section content

This task swaps the hash basis end to end so the ADR-0024 byte-equal invariant between snapshot-seeded hashes and loop-tick hashes is preserved across the change. Both `snapshot_and_seed` and `compute_pushes` are updated in one commit because the invariant binds them.

**Files:**
- Modify: `src/autosync.rs` (`compute_pushes`, `snapshot_and_seed`, delete `rendered_oob_with_hash` and `stable_hash`, add `section_content_hash` and its parity test)

**Interfaces:**
- Produces: `fn section_content_hash(scan: &RootScan) -> u64`. Module-private. Consumed by `compute_pushes` and `snapshot_and_seed`.
- Removed: `fn rendered_oob_with_hash(...) -> (String, u64)`, `fn stable_hash(&str) -> u64`.

- [ ] **Step 1: Write the parity contract test**

Add this test inside the existing `#[cfg(test)] mod tests { ... }` block in `src/autosync.rs`, alongside `rendered_oob_with_hash_returns_render_oob_section_paired_with_its_stable_hash`. After this task completes, the existing `rendered_oob_with_hash_returns_render_oob_section_paired_with_its_stable_hash` test is also deleted because the function it pins no longer exists; the new test below pins the replacement contract.

```rust
#[test]
fn content_hash_equals_render_parity() {
    // Equality of section_content_hash must imply equality of rendered HTML, so
    // compute_pushes can skip the render on a hash match without dropping a
    // real diff. Fails closed if a future renderer input lands outside the
    // section tuple.
    let a = walked_root_with_folder(0, true);
    let b = walked_root_with_folder(0, true);
    let links = no_links();

    assert_eq!(section_content_hash(&a), section_content_hash(&b));
    assert_eq!(
        render_oob_section(&a, 0, ViewMode::GapsOnly, &links),
        render_oob_section(&b, 0, ViewMode::GapsOnly, &links),
    );

    // Flip one bit of content
    let c = walked_root_with_folder(0, false);
    assert_ne!(section_content_hash(&a), section_content_hash(&c));
    assert_ne!(
        render_oob_section(&a, 0, ViewMode::GapsOnly, &links),
        render_oob_section(&c, 0, ViewMode::GapsOnly, &links),
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p missing-ebooks --lib autosync::tests::content_hash_equals_render_parity`

Expected: compile error, "cannot find function `section_content_hash` in this scope".

- [ ] **Step 3: Replace `stable_hash` and `rendered_oob_with_hash` with `section_content_hash`**

Delete the `stable_hash` function at L53-60 and the `rendered_oob_with_hash` function at L78-90.

Insert `section_content_hash` in their place. Place it directly after `render_oob_section`:

```rust
/// Hashes one section's content for the autosync dedup compare. Shared by
/// `snapshot_and_seed` (the seed hash a new subscriber carries) and
/// `compute_pushes` (the per-tick compare), so the seed and the first-tick
/// hash agree by construction (ADR-0024). Match implies equal rendered HTML;
/// the `content_hash_equals_render_parity` test pins that contract.
fn section_content_hash(scan: &crate::scanner::RootScan) -> u64 {
    let mut hasher = DefaultHasher::new();
    scan.hash(&mut hasher);
    hasher.finish()
}
```

The `use` for `std::hash::Hash` already exists at the top of the file (it backs `stable_hash`'s `s.hash(&mut hasher)`); the `use` for `DefaultHasher` likewise stays. If either ends up unused after the deletions, clippy in Step 8 will surface it.

- [ ] **Step 4: Rewrite `compute_pushes`**

Replace the entire body of `compute_pushes` (L20-51) with the spec's content-hash form. The loop nesting flips: root is now the outer loop so the content hash is computed once per root, not once per `(root, mode)`. The `has_subs[mode]` and `resize` checks move inside; both are O(1) and the placement keeps the per-mode skip next to its hash compare.

```rust
/// Diff each section's content hash against `last_content_hash` and return the
/// list of pushes, mutating `last_content_hash` in place. On a hash match the
/// loop skips the render entirely; on a miss it renders, updates the cache,
/// and pushes. The hash is hoisted above the mode loop because the section
/// content does not depend on `ViewMode`.
///
/// `has_subs[mode]` short-circuits modes nobody is listening to: their hashes
/// stay untouched and they produce no pushes.
fn compute_pushes(
    raw: &raw_view::RawView,
    last_content_hash: &mut EnumMap<ViewMode, Vec<Option<u64>>>,
    has_subs: EnumMap<ViewMode, bool>,
    links: &[crate::config::SearchLink],
) -> Vec<(ViewMode, usize, String)> {
    let mut pushes = Vec::new();
    for (root_idx, section) in raw.iter().enumerate() {
        let content_hash = section_content_hash(section);
        for mode in [ViewMode::GapsOnly, ViewMode::All] {
            if !has_subs[mode] {
                continue;
            }
            // Roots are config-fixed for a process, but the first call has
            // empty vecs.
            if last_content_hash[mode].len() != raw.len() {
                last_content_hash[mode].resize(raw.len(), None);
            }
            if last_content_hash[mode][root_idx] == Some(content_hash) {
                continue;
            }
            let html = render_oob_section(section, root_idx, mode, links);
            last_content_hash[mode][root_idx] = Some(content_hash);
            pushes.push((mode, root_idx, html));
        }
    }
    pushes
}
```

- [ ] **Step 5: Rewrite `snapshot_and_seed`**

Replace the loop body in `snapshot_and_seed` (L98-108). The snapshot still renders every section once because the payload IS the snapshot; what changes is that the seed hash is now derived from section content, not from the rendered string.

```rust
/// Build the concatenated OOB-swap payload for an SSE `snapshot` event and the
/// per-root content hashes the autosync loop will use to suppress redundant
/// first-tick section events. The handler sends the payload, then passes the
/// hashes to `Autosync::subscribe` so the loop's first compute_pushes finds
/// matching hashes and emits nothing until something actually changes.
fn snapshot_and_seed(
    raw: &raw_view::RawView,
    mode: ViewMode,
    links: &[crate::config::SearchLink],
) -> (String, Vec<u64>) {
    let mut payload = String::with_capacity(raw.len() * 512);
    let mut hashes = Vec::with_capacity(raw.len());
    for (root_idx, section) in raw.iter().enumerate() {
        let oob = render_oob_section(section, root_idx, mode, links);
        hashes.push(section_content_hash(section));
        payload.push_str(&oob);
    }
    (payload, hashes)
}
```

- [ ] **Step 6: Rename `last_hash` to `last_content_hash` at remaining call sites**

`compute_pushes`'s parameter was renamed in Step 4. The field on the registry (search for `last_hash:` in `AutosyncInner` and friends, plus the `Subscriber` row's seed slot) keeps the same shape but the name changes to `last_content_hash`. Update every reference in `src/autosync.rs`. Existing tests that build registries by hand (e.g. `empty_hashes` at L392) need the parameter name updated too if they spell it.

Confirm with: `rg -n '\blast_hash\b' src/autosync.rs`. Expected: no matches after this step.

- [ ] **Step 7: Delete the obsolete pair test**

Remove `rendered_oob_with_hash_returns_render_oob_section_paired_with_its_stable_hash` (currently at L782-807). Its pair is gone; `content_hash_equals_render_parity` is the replacement contract.

- [ ] **Step 8: Run the parity test, the autosync module, and the full suite**

```bash
cargo test -p missing-ebooks --lib autosync::tests::content_hash_equals_render_parity
cargo test -p missing-ebooks --lib autosync::tests
cargo test -p missing-ebooks
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

Expected: all green. The existing behavior tests (`identical_second_call_pushes_nothing`, `changed_root_produces_pushes_only_for_that_root`, the snapshot/subscribe lifecycle tests, `oob_byte_equality_snapshot_vs_tick` if it exists in the integration suite) continue to pass because they assert on which events arrive on which channels, not on the hash basis.

- [ ] **Step 9: Commit**

```bash
git add src/autosync.rs
git commit -m "feat(autosync): switch dedup basis from rendered HTML to section content

Replace rendered_oob_with_hash and stable_hash with one
section_content_hash that hashes scanner::RootScan via its derived Hash
impl. compute_pushes hoists the hash above the per-mode loop and skips
the render entirely on a hash match; on no-change ticks the autosync
loop now does zero render work. snapshot_and_seed produces the same
hash for the seed so ADR-0024's seed-equals-first-tick invariant
holds. content_hash_equals_render_parity pins the renderer-purity
contract that lets the cache skip the render safely.

Measured savings on no-change ticks (one root, both modes subscribed,
from benches/render.rs): ~6 ms at 1k folders, ~67 ms at 10k, ~440 ms
at 50k. Paid forever while a tab stays open."
```

---

### Task 3: Instrument render count on `AutosyncInner`

The spec describes a `#[cfg(test)]` field with a `#[cfg(test)]` accessor; in practice the reference pattern in `RawViewStore` (commit `7e5254e`) keeps the field unconditional and gates only the accessor. Matching that pattern avoids cluttering every `AutosyncInner` constructor with `cfg(test)`. The atomic field is one machine word and one relaxed `fetch_add` per render; the cost is negligible in production.

The counter must capture both snapshot renders (from `snapshot_and_seed`) and per-tick renders (from `compute_pushes`), so the spec's first test can read the snapshot floor before driving ticks. Both call sites bump the counter by the known render count after the helper returns; the helpers themselves stay pure.

**Files:**
- Modify: `src/autosync.rs` (add `render_count` field on `AutosyncInner`, add `cfg(test)` accessor on `Autosync`, bump from `run_loop` after `compute_pushes` and from the `attach`/snapshot path after `snapshot_and_seed`)

**Interfaces:**
- Produces: `AtomicU64` render counter on `AutosyncInner`, accessible via `Autosync::render_count` under `#[cfg(test)]`. Task 4 consumes it.

- [ ] **Step 1: Add the counter field**

Add to `AutosyncInner`:

```rust
/// Monotonic count of every render_oob_section call observed by the
/// autosync paths (snapshot seed and per-tick loop). Tests diff before
/// vs. after to assert that no-change ticks skip the render. Mirrors
/// `RawViewStore::rebuild_count` (state.rs:54).
render_count: AtomicU64,
```

Initialize to `AtomicU64::new(0)` wherever `AutosyncInner` is constructed. Add a `use std::sync::atomic::{AtomicU64, Ordering};` import if `AutosyncInner` does not already pull these in.

- [ ] **Step 2: Add the test accessor**

In `impl Autosync`:

```rust
#[cfg(test)]
pub fn render_count(&self) -> u64 {
    lock_inner(&self.inner).render_count.load(Ordering::Relaxed)
}
```

`lock_inner` is the existing poison-recovering helper in `src/autosync.rs` (it's the one the spec calls out at L283/295/332/344/353 in the audit, replaced in commit `3274608`). If `lock_inner`'s signature differs, adapt the call; the contract is "acquire the inner lock with poison recovery, read the counter, release."

- [ ] **Step 3: Bump from the per-tick path**

In `run_loop`, after `compute_pushes` returns, bump by `pushes.len()`. The new `compute_pushes` body renders exactly once per pushed `(mode, root)` pair, so `pushes.len()` is the exact render count for that tick.

```rust
let pushes = compute_pushes(&raw, &mut inner.last_content_hash, has_subs, &links);
inner.render_count.fetch_add(pushes.len() as u64, Ordering::Relaxed);
```

Adapt the local binding names to whatever `run_loop` actually uses; the contract is one `fetch_add` of `pushes.len()` per `compute_pushes` call.

- [ ] **Step 4: Bump from the snapshot path**

In the path that calls `snapshot_and_seed` (search for `snapshot_and_seed(` in `src/autosync.rs`; the call site is inside `attach`), bump by `hashes.len()` after the call. `snapshot_and_seed` renders one section per root, so `hashes.len()` (which equals `raw.len()`) is the exact render count.

```rust
let (payload, hashes) = snapshot_and_seed(&raw, mode, &links);
lock_inner(&self.inner).render_count.fetch_add(
    hashes.len() as u64,
    Ordering::Relaxed,
);
```

If the call site already holds the inner lock, fold the bump into the existing guard; do not re-lock.

- [ ] **Step 5: Run the autosync module and clippy**

```bash
cargo test -p missing-ebooks --lib autosync::tests
cargo clippy --all-targets -- -D warnings
```

Expected: all green. No behavior change in production (one `fetch_add` per render path; relaxed ordering).

- [ ] **Step 6: Commit**

```bash
git add src/autosync.rs
git commit -m "feat(autosync): instrument render count on AutosyncInner

Mirrors the rebuild_count pattern added to RawViewStore in 7e5254e.
Field is unconditional, accessor is cfg(test). Bumped from run_loop
(by pushes.len()) and from the attach snapshot path (by hashes.len())
so tests can read the snapshot floor before driving ticks and prove
that no-change ticks skip the render entirely."
```

---

### Task 4: Assert render avoidance via `render_count`

**Files:**
- Modify: `src/autosync.rs::tests` (one new loop test, one new compute_pushes-level test)

**Interfaces:**
- Consumes: `Autosync::render_count` from Task 3, plus the existing `test_state_with_interval`, `subscribe`, `walked_root_with_folder`, `raw_view_of`, and `abort_loop_for_test` helpers in the test module.

The loop path is the one where `render_count` is visible end-to-end. Direct `compute_pushes` calls bypass `run_loop`'s increment, so the "only changed root re-rendered" assertion stays at the existing `changed_root_produces_pushes_only_for_that_root` level (via `pushes.len()`) and the new test layered on top adds a content-hash invariant rather than a render-count one.

- [ ] **Step 1: Write `no_change_tick_does_not_render`**

Attach a subscriber per mode (snapshot fires once per attach, bumping `render_count` by `roots` each), then drive several ticks against the unchanged scan and assert the count did not grow. The existing tests use `subscribe`, not `attach`; the snapshot path in the codebase is whichever one calls `snapshot_and_seed`. Use the same helper the existing tests use.

```rust
#[tokio::test]
async fn no_change_tick_does_not_render() {
    // 10ms interval, same shape as test_state_with_interval(0) callers but
    // with a tick fast enough that several fire during the sleep.
    let state = test_state_with_interval(1);
    let (tx_gaps, _rx_gaps) = mpsc::channel(8);
    let (tx_all, _rx_all) = mpsc::channel(8);
    state.autosync.subscribe(&state, ViewMode::GapsOnly, tx_gaps);
    state.autosync.subscribe(&state, ViewMode::All, tx_all);

    // Snapshot floor: snapshot_and_seed ran once per subscribe, each render
    // produced one section per root. The scan is the curated `clean-error`
    // scenario from test_state_with_interval; count its roots once and
    // reuse the figure.
    let snapshot_floor = state.autosync.render_count();
    assert!(snapshot_floor > 0, "snapshot rendered at least one section");

    // Drive several ticks against the unchanged scan
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert_eq!(
        state.autosync.render_count(),
        snapshot_floor,
        "no-change ticks must not render",
    );
}
```

The `1`-second interval matches the smallest config; if the existing test infrastructure supports sub-second intervals (look at the `autosync_interval_seconds` field shape), prefer a faster tick and a shorter sleep so the test is not slow.

- [ ] **Step 2: Write `content_change_targets_only_changed_root`**

This test stays at the `compute_pushes` level because the loop path requires filesystem mutation to surface a scan change, which the existing test infrastructure does not support. The existing `changed_root_produces_pushes_only_for_that_root` (L477) already proves the targeting; the new test adds the content-hash invariant directly.

```rust
#[test]
fn content_change_targets_only_changed_root() {
    // After a content change on root 0, the second compute_pushes call sees
    // a content-hash mismatch on root 0 and a match on root 1. The new
    // pushes contain root 0 only.
    let raw_before = raw_view_of(vec![
        walked_root_with_folder(0, true),
        walked_root_with_folder(1, true),
    ]);
    let mut hashes = empty_hashes();
    let links = no_links();
    let _seed = compute_pushes(&raw_before, &mut hashes, both_modes_subscribed(), &links);

    // Flip missing_ebook on root 0 only
    let raw_after = raw_view_of(vec![
        walked_root_with_folder(0, false),
        walked_root_with_folder(1, true),
    ]);
    let pushes = compute_pushes(&raw_after, &mut hashes, both_modes_subscribed(), &links);

    let touched_roots: std::collections::BTreeSet<usize> =
        pushes.iter().map(|(_, root, _)| *root).collect();
    assert_eq!(touched_roots, std::collections::BTreeSet::from([0]));
    // Both modes are subscribed, so the changed root produced one push per mode.
    assert_eq!(pushes.len(), 2);
}
```

If `changed_root_produces_pushes_only_for_that_root` already covers this exact shape after Task 2's rewrite, fold the new assertions into it instead of adding a duplicate test.

- [ ] **Step 3: Run the new tests in isolation, then the module, then the suite**

```bash
cargo test -p missing-ebooks --lib autosync::tests::no_change_tick_does_not_render
cargo test -p missing-ebooks --lib autosync::tests::content_change_targets_only_changed_root
cargo test -p missing-ebooks --lib autosync::tests
cargo test -p missing-ebooks
```

Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add src/autosync.rs
git commit -m "test(autosync): assert no-change ticks skip render

no_change_tick_does_not_render goes through subscribe + tokio sleep to
prove render_count does not grow once the snapshot has run.
content_change_targets_only_changed_root pins the new content-hash
basis: a change on one root invalidates only that root's cache entry."
```

---

### Task 5: Document the dedup basis in ADR-0024

**Files:**
- Modify: `docs/adr/0024-autosync-section-level-oob-swap.md`

- [ ] **Step 1: Add the dedup-basis paragraph to "Consequences"**

Append after the existing "byte-equal invariant tested by `tests/cache_render_byte_equal.rs` ..." paragraph (currently the last paragraph of "Consequences"):

```markdown
The per-broadcast dedup is computed from the section content (`scanner::RootScan` via its derived `Hash`), not the rendered HTML. Equality of content hash implies equality of rendered output because the renderer is pure on `(section, root_idx, mode, links)` and the three non-section inputs are loop-stable for one autosync registry. The `content_hash_equals_render_parity` test in `src/autosync.rs::tests` pins this contract: any future renderer input outside the section breaks this test before it can let the cache silently go stale.
```

- [ ] **Step 2: Verify `cargo doc -D warnings` is clean**

The ADR is markdown, not rustdoc, but the pre-commit hook also runs `cargo doc -D warnings`. Run it to confirm nothing else regressed:

```bash
cargo doc --no-deps -D warnings
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add docs/adr/0024-autosync-section-level-oob-swap.md
git commit -m "docs(adr): note autosync dedup basis is section content (ADR-0024)

Records the renderer-purity invariant that lets compute_pushes skip
the render on a content-hash match. content_hash_equals_render_parity
is the test that catches any future renderer input outside the
section tuple."
```

---

### Task 6: Verify against the audit harness

This is a sanity pass, not a commit. The CLAUDE.md "Verifying UI changes" section calls out `examples/explore.rs` as the harness for visual verification. This change is server-side and does not touch HTML, so a UI walk-through is not required, but the autosync push path is the one this change affects most.

- [ ] **Step 1: Start the harness if available and unloaded**

```bash
lsof -iTCP:8919 -sTCP:LISTEN
```

If the port is free, run:

```bash
cargo run --example explore -- big-library --port 8919
```

If the port is taken, pick another and adjust. The `big-library` scenario is the relevant one for this change because the dedup-basis swap only shows a measurable difference at scale.

- [ ] **Step 2: Hand a clickable URL to the user**

Surface the URL so the user can confirm autosync still updates sections when the underlying scan changes (mark a folder, observe the section refresh) and produces no noise when the scan is stable.

- [ ] **Step 3: Stop the harness when done**

Match by working directory to avoid taking down a peer agent's instance:

```bash
for pid in $(pgrep -f examples/explore); do
  cwd=$(readlink /proc/$pid/cwd 2>/dev/null)
  [ "$cwd" = "$(pwd)" ] && kill "$pid"
done
```
