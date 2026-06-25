# Memoize per-section renders in `autosync::compute_pushes`

From the 2026-06-25 audit (`deep-dive/missing-ebooks-audit-2026-06-25.md`, "Worth doing" item #5). The earlier "Fix now" items (#1 autosync poisoning recovery, #2 graceful shutdown, #3 strict env parsing) shipped under `.scratch/lifecycle-hardening/`. Item #4 (feature-gate `scenarios.rs`) was explicitly deferred there as cosmetic for an unpublished binary. This is the next un-actioned item.

## Why

`autosync::compute_pushes` is the per-tick fan-out path that decides which SSE subscribers receive a section-level OOB swap (ADR-0024). Today its work shape is:

1. For each subscribed `(mode, root_idx)`, render the section to its OOB-swap HTML string.
2. Hash the rendered string with `DefaultHasher`.
3. If the hash differs from `last_hash[mode][root_idx]`, push the new HTML and update the cache.

The hash is computed from the render output, so the render runs on every tick regardless of whether the underlying scan changed. With `interval` defaulting to a few seconds and a tab open, the loop renders `2 × roots` sections per tick into discarded strings whenever nothing on disk has changed. ADR-0022's "per-folder render cost is microseconds" still applies, but for a flagship 50k-folder library that aggregates to tens to hundreds of ms per tick on a runtime worker; for a typical 1k-folder home setup the cost is well under a ms but is paid forever for as long as a tab stays open.

The audit's recommendation is to flip the hash basis: compute the hash from the raw section content, not the rendered HTML. On a content-hash match the loop skips the render entirely and pushes nothing. Equality of content hash implies equality of rendered HTML because the renderer is pure on `(section, root_idx, mode, links)` and `root_idx`, `mode`, and `links` are loop-stable. The audit phrases this as "bigger impact than moving render to `spawn_blocking`".

The size of the win is bounded by the renderer cost itself, which is unmeasured in this codebase (the audit flagged its own render-risk language as speculation without a benchmark). Item #7 in the same audit asks for a `criterion` bench on `render_view`. That bench is the right place to quantify the savings and protect them. This spec deliberately ships #5 without #7: the worst case is a wash and the test discipline below catches behavioral regressions. The bench is a separate item.

## End state

`compute_pushes` computes one `section_content_hash(&RootScan) -> u64` per root per tick, hoisted above the per-mode loop because the hash is mode-independent. On a match against `last_content_hash[mode][root_idx]`, the loop skips both the render and the push for that `(mode, root)`. On a miss, it renders, updates the cache, and pushes.

The registry field `last_hash` is renamed `last_content_hash` so the field name carries the semantics. Its shape (`EnumMap<ViewMode, Vec<Option<u64>>>`) is unchanged. The seed-hash API on `subscribe_and_seed` is unchanged at the type level: it still takes `Vec<u64>`. What changed is what the u64 means; the snapshot path now computes content hashes for the seed rather than rendered-HTML hashes.

The helper `rendered_oob_with_hash` is deleted. Its two callers split:

- `snapshot_and_seed` renders the OOB string for the snapshot payload as before, and computes `section_content_hash` for the seed independently.
- `compute_pushes` computes `section_content_hash` first and only calls `render_oob_section` on a miss.

`stable_hash` (the `DefaultHasher` helper over `&str`) is gone. `section_content_hash` uses the same `DefaultHasher` shape over the structural `Hash` impl described below.

The scanner types `RootScan` and `ScannedFolder` gain `Hash` in their derive lists. `PathBuf`, `bool`, `String`, and `Vec<T: Hash>` all implement `Hash` already, so the derive compiles without further changes. The two types are simple data so the public-API commitment of adding a `Hash` bound is minor.

A `#[cfg(test)] render_count: AtomicU64` is added to `AutosyncInner` alongside `loop_task`, with a `#[cfg(test)] pub fn render_count(&self) -> u64` accessor. The render path increments it inside a thin test-only wrapper around `render_oob_section` (mirroring the `rebuild_count` pattern added in commit `7e5254e` for `RawViewStore`).

## Data flow

Per tick, `run_loop` calls `state.store.refresh().await` unchanged. The new `compute_pushes` body is:

```rust
for (root_idx, section) in raw.iter().enumerate() {
    let content_hash = section_content_hash(section);
    for mode in [ViewMode::GapsOnly, ViewMode::All] {
        if !has_subs[mode] {
            continue;
        }
        // Resize on first call: roots are config-fixed but the vec starts empty.
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
```

The hoist of the hash above the mode loop is intentional: the section content does not depend on `ViewMode`, so computing it once per root halves the per-tick hash work compared to the literal "swap one hash for another" version. The `resize` and `has_subs` checks move inside the root loop; both are cheap and the rearrangement keeps the per-mode skip near its hash compare for legibility.

`snapshot_and_seed` becomes:

```rust
for (root_idx, section) in raw.iter().enumerate() {
    let oob = render_oob_section(section, root_idx, mode, links);
    hashes.push(section_content_hash(section));
    payload.push_str(&oob);
}
```

The snapshot still renders every section once (it has to: the payload is the snapshot). The change is purely on the seed side. ADR-0024's "byte-equal seed hash = first tick hash" invariant continues to hold: both paths now derive the seed and the per-tick compare from the same `section_content_hash` function.

No change to `run_loop`, `subscribe_and_seed`, `attach`, `try_exit_loop`, `lock_inner`, or `section_event`. No change to `state.rs`, `raw_view.rs`, or `web/render.rs`.

## Tests

Three new tests in `src/autosync.rs::tests`, all leveraging the existing `abort_loop_for_test` plumbing and the curated scenario fixtures the module already uses:

- **`no_change_tick_does_not_render`**: build a deterministic raw view, attach a subscriber, drive N additional ticks against an unchanged scan, assert `render_count` is exactly `roots * subscribed_modes` (the seed renders only) after the loop settles. The seed comes from `snapshot_and_seed`, which renders for the payload; the per-tick path renders zero times.
- **`content_change_re_renders_only_changed_root`**: same setup, then mutate one section's content (e.g., flip a `missing_ebook` flag on one folder), drive one tick, assert `render_count` grew by exactly `subscribed_modes` (one render per subscribed mode for the one changed root) and untouched roots produced no render.
- **`content_hash_equals_render_parity`**: pure unit test, no loop. Build two `RootScan` values with structurally-equal content, assert `section_content_hash(a) == section_content_hash(b)` and `render_oob_section(a, ..) == render_oob_section(b, ..)`. Mutate one field in `b`, assert both hashes differ and the rendered HTML differs. This is the contract that lets `compute_pushes` skip the render safely; if the renderer ever gains an input outside the section, this test fails before the cache silently goes stale.

The existing autosync test surface (`first_tick_suppresses_seeded_sections`, `later_subscriber_does_not_overwrite_baseline`, `oob_byte_equality_snapshot_vs_tick`, the abort-and-respawn cases) continues to pass without modification: it asserts on user-visible behavior (which events arrive on which channels), not on the hash basis.

## ADR

ADR-0024 ("Autosync pushes are per-root section OOB swaps") gains a short paragraph noting the dedup basis. The change is one paragraph added to "Consequences", along these lines:

> The per-broadcast dedup is now computed from the section content (`scanner::RootScan` via its derived `Hash`), not the rendered HTML. Equality of content hash implies equality of rendered output because the renderer is pure on `(section, root_idx, mode, links)` and the three non-section inputs are loop-stable. The `content_hash_equals_render_parity` test in `src/autosync.rs::tests` pins this contract: any future renderer input outside the section breaks this test before it can let the cache silently go stale.

No new ADR. The substrate decisions (ADR-0022 on cache-holds-raw-scan-output, ADR-0027 on substrate consolidation) are unaffected.

## Out of scope

Each of these is its own audit item or its own deferred item; none belongs in this spec:

- Moving `render_oob_section` to `spawn_blocking` (a separate flag in the audit, mentioned by item #5 only as a comparison; deferred because the cache skip removes the need on no-change ticks and the per-section render on change ticks is bounded).
- Teaching `RawViewStore::refresh()` to return the existing `Arc<RawView>` on no-op rebuilds (option A in the brainstorm; deferred because it crosses ADR-0027's tripwire and the section-content cache captures the same savings).
- A `criterion` bench on `render_view` (audit #7; its own work item, the right place to quantify this change's win).
- Sharding the `DirIndex` per root (audit #9; strategic, deferred).
- Replacing `strip_prefix(root).ok()` at `scanner.rs:454,564` with `debug_assert!` (audit #6; small, independent, its own work item).

## Risk

One risk worth naming: a future renderer change that adds an input outside the section (a config knob read per render, a clock-derived field) would break the content-hash equivalence silently. Content hash would match across ticks but rendered output would diverge, and subscribers would see stale section HTML until something else invalidated the cache.

The mitigations are mechanical:

- `content_hash_equals_render_parity` catches the most common shape at PR time.
- The ADR-0024 paragraph documents the renderer-input invariant so future changes have a place to read about the constraint before adding a new input.

The `O_EXCL` and TOCTOU items from the audit's "Defer or accept" bucket are outside this spec entirely.
