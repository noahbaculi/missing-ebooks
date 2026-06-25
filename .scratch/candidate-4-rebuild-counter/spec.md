# Replace `Arc::ptr_eq` freshness asserts with a rebuild counter

Architecture review Candidate 4, from `.scratch/architecture-review/findings.md`.

## Why

`RawViewStore` carries a `#[cfg(test)]` accessor `peek_stored_arc()` that hands back an `Arc<RawView>` clone so tests can `assert!(Arc::ptr_eq(&before, &after))` to prove a warm read did not rebuild the cache slot. Five tests in `state.rs::tests` (six `Arc::ptr_eq` calls in total) lean on this pattern.

The property those tests want is real and load-bearing: ADR-0022 says a warm read returns the stored entry without rebuilding, and `RawViewStore::write_mark` relies on `Arc::make_mut` to mutate the slot in place. The way the tests express that property is brittle. `Arc::ptr_eq` returns true today only because the slot holds the unique strong reference at edit time, which is an implementation accident, not a contract of `current()` / `write_mark`. Any future change that holds a second `Arc<RawView>` across the slot lock (a tracing hook capturing the view for a debug field, a metrics sampler computing a count, a future read endpoint that clones from the slot) flips `Arc::make_mut` from in-place mutation to reallocation, the allocation address moves, and the test fails. The failure message would still say "warm reads must not rebuild" and point at the slot, sending the reader to the wrong module: the cause lives at the new strong-ref holder, not at the store.

The fix is to express the property in its own terms: count how many fresh builds the store has stored into the slot, and have the tests assert that the count did not change across a warm operation. The counter increments at exactly one site (`store_fresh`, which is the only place a fresh build is written to the slot), so the bookkeeping is precise.

## End state

`RawViewStore` gains one atomic field and one test-only accessor. The slot accessor `peek_stored_arc` is deleted.

| Item | Change |
| --- | --- |
| `rebuild_count: AtomicU64` | New field on `RawViewStore`, initialized to `0` in `new()`. |
| `store_fresh` | Becomes a method on `&RawViewStore` so it can bump the counter alongside the slot write. Increments `self.rebuild_count` with `fetch_add(1, Ordering::Relaxed)` after seating the entry. Behavior is otherwise unchanged: same `stored_at = Instant::now()`, same `Arc::new(raw)`, same return. |
| `rebuild_count()` | New `#[cfg(test)] pub fn rebuild_count(&self) -> u64`. Sync, lock-free, returns `self.rebuild_count.load(Ordering::Relaxed)`. |
| `peek_stored_arc()` | Removed. The only callers are the five tests rewritten below. |

The five production call sites of `store_fresh` (`current`, `refresh`, `rescan`, plus `write_mark`'s cold path and `remove_mark`'s cold path) change from `store_fresh(&mut slot, raw)` to `self.store_fresh(&mut slot, raw)`. No other production code changes. `Applied`, `DomainError`, `RawView`, the demo, autosync, and the web handlers are untouched.

Doc text for the new items follows the `writing-style-code-comments` skill (verb-first for the accessor, noun-phrase for the field, backticks around identifiers, no em dashes, no "This function" opener, terse):

```rust
/// Monotonic count of fresh builds stored into the slot. Bumped inside
/// `store_fresh`. Test-only observation; tests diff before vs. after to
/// assert that a warm operation did not rebuild. See ADR-0022.
rebuild_count: AtomicU64,
```

```rust
/// Returns the count of fresh builds stored into the slot since this store
/// was created.
#[cfg(test)]
pub fn rebuild_count(&self) -> u64 {
    self.rebuild_count.load(Ordering::Relaxed)
}
```

The inline `// dir index test accessor` block at `state.rs:146-151` is unrelated and stays.

## Test rewrites

Five tests in `src/state.rs::tests`, all in place. No test moves and no new test is added. Every captured baseline binds the same name (`rebuilds_before`) so the post-op assert reads cleanly.

| Test | Today | After |
| --- | --- | --- |
| `store_current_serves_stored_raw_within_ttl` (`:454`) | `let first = store.current().await; ... let second = store.current().await; assert!(Arc::ptr_eq(&first, &second), ...);` | `store.current().await; let rebuilds_before = store.rebuild_count(); ... store.current().await; assert_eq!(store.rebuild_count(), rebuilds_before, "warm read must not rebuild");` |
| `store_current_single_flights_a_cold_slot` (`:467`) | `let (a, b) = tokio::join!(s1.current(), s2.current()); assert!(Arc::ptr_eq(&a, &b), ...);` | `let (_a, _b) = tokio::join!(s1.current(), s2.current()); assert_eq!(store.rebuild_count(), 1, "single-flight: one rebuild for two concurrent cold reads");` |
| `store_write_mark_edits_the_slot_in_place` (`:531`) | warm `current()`, `write_mark`, `peek_stored_arc`, `assert!(Arc::ptr_eq(&stored, &applied.raw), ...)` | warm `current()`, `let rebuilds_before = store.rebuild_count();`, `write_mark` (assert `created`), `assert_eq!(store.rebuild_count(), rebuilds_before, "warm write_mark did not rebuild")`, `assert!(!book_missing(&applied.raw), "the edit is reflected")`, follow-up `current()`, `assert_eq!(store.rebuild_count(), rebuilds_before, "follow-up read did not rebuild")`, `assert!(!book_missing(&next), "follow-up read reflects the mark")` |
| `store_warm_concurrent_reads_share_one_raw_slot` (`:686`) | warm `current()`, `peek`, `join!`, `Arc::ptr_eq(&a, &b)`, second `peek`, `Arc::ptr_eq(&before, &after)` | warm `current()`, `let rebuilds_before = store.rebuild_count();`, `join!(current, current)`, `assert_eq!(store.rebuild_count(), rebuilds_before, "warm concurrent reads must not rebuild")` |
| `store_write_mark_warm_slot_survives_follow_up_read` (`:799`) | warm `current()`, `write_mark`, two `peek_stored_arc` calls flanking a `current()`, `Arc::ptr_eq(&raw_after_mark, &raw_after_read)` | warm `current()`, `let rebuilds_before = store.rebuild_count();`, `write_mark` (assert mark visible), `assert_eq!(store.rebuild_count(), rebuilds_before, "warm write_mark did not rebuild")`, second `current()`, `assert_eq!(store.rebuild_count(), rebuilds_before, "follow-up read did not rebuild")`, assert mark still visible |

After this, no test in `state.rs` (or anywhere else; `service.rs` is gone after ADR-0028) calls `Arc::ptr_eq` on an `Arc<RawView>` or calls `peek_stored_arc`. The `Arc::make_mut`-in-place edit invariant is named at the level the test cares about: "a warm `write_mark` does not rebuild the slot, and the next read reflects the mark." That is exactly what the production code guarantees.

The first test in the list (`store_current_serves_stored_raw_within_ttl`) drops its `first` and `second` bindings because nothing reads them after the assert. The second test (`store_current_single_flights_a_cold_slot`) keeps `_a` and `_b` to hold the futures and drives both arms; the counter alone makes the assert.

## Why no second strong-ref holder appears in this change

The counter expresses the right property without depending on `Arc::ptr_eq`. Adding the counter is independent of whether such a holder exists today; the change buys correctness at the test-failure-message level for any future holder that lands. No production code in this spec introduces such a holder.

## Files touched

- Modify: `src/state.rs` (one new field, one new accessor, `store_fresh` rehomed as a method, `peek_stored_arc` removed, five tests rewritten, `use std::sync::atomic::{AtomicU64, Ordering};` added).

No new files. No ADR. ADR-0022 (warm reads must not rebuild) and ADR-0027 (substrate consolidated behind `RawViewStore`) are upheld more precisely, not changed. The counter is test mechanics, not a design decision the codebase needs to remember; an ADR would be ceremony.

## Commit plan

Granular, conventional, no squash. Build green at every commit.

1. `refactor(state): add rebuild_count to RawViewStore`. New `AtomicU64` field, `store_fresh` becomes a method on `&RawViewStore` and bumps the counter, new `#[cfg(test)] pub fn rebuild_count(&self) -> u64`, five call sites of `store_fresh` rewired. `peek_stored_arc` and the five `Arc::ptr_eq` tests still compile and pass; no test changes yet.
2. `chore(tests): assert via rebuild_count instead of Arc::ptr_eq`. Rewrite the five tests in `state.rs::tests` per the table above. `peek_stored_arc` still present; no production change.
3. `refactor(state): drop peek_stored_arc`. Delete the `#[cfg(test)] pub async fn peek_stored_arc` method now that no caller remains. The `dir_index` test accessor stays.

After each commit: `cargo test`, plus the pre-commit hook (fmt, clippy, `cargo doc -D warnings`, accent test when assets or accent tests change). Splitting the change into three commits keeps each one mechanical: a field add, a test rewrite, a dead-code delete.

## Out of scope

- Adding any production reader for `rebuild_count` (telemetry, metrics, tracing field, autosync). The accessor stays `#[cfg(test)]`; YAGNI.
- Removing the `dir_index` test accessor at `state.rs:149`. Separate concern, independent property.
- Candidates 5 (the `/mark` error path re-fetch) and 6 (the `Autosync::subscribe` trio). Separate specs.
- Any change to `Applied`, `DomainError`, `RawView`, or any public surface of `RawViewStore`.

## Non-goals

- No production behavior change. `current` / `refresh` / `rescan` / `write_mark` / `remove_mark` return the same values in the same order, on the same locks, with the same allocation behavior.
- No new dependencies.
- No new public crate surface. The counter accessor is `#[cfg(test)]`, like the slot accessor it succeeds.
- No ADR. The change refines how an existing invariant is checked, not what the invariant is.

## Constraints

- Comments follow the `writing-style-code-comments` style: verb-first or noun-phrase doc summaries, terse, no "This function" openers, no em dashes, backticks around identifiers and literals. Inline comments are imperatives or bare label noun phrases.
- Prose in this spec and the implementation plan follows the `humanizer` skill. No em dashes anywhere.
- Conventional Commits (`type(scope): subject`), no squash, no `--no-verify`.
- After each commit: `cargo test` must pass. Pre-commit hook runs fmt, clippy, `cargo doc -D warnings`, and the accent test for asset or accent changes.
