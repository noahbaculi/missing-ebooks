# Autosync: shared `(oob_html, hash)` pair for sections

A scoped-down realization of architecture-review candidate #7. After #3 (`Autosync::attach`) shipped, the original "single `RawView::sections(mode)` iterator" framing is mostly already captured by `service::render_section_from_raw`. What remains is one localized duplication inside `src/autosync.rs`: the `render_oob_section` + `stable_hash` lockstep that appears in both `snapshot_and_seed` and `compute_pushes`. This spec concentrates that pair behind one private helper.

## Background

`autosync.rs` renders a section into its OOB-swap HTML and hashes that HTML in two places:

`snapshot_and_seed` (L92-95) seeds the per-root baseline hashes that the registry hands to the loop, so the loop's first tick can suppress redundant section events for sections the snapshot already carried:

```rust
let oob = render_oob_section(section, root_idx, mode, links);
hashes.push(stable_hash(&oob));
payload.push_str(&oob);
```

`compute_pushes` (L43-47) renders and hashes every section every tick, diffing against `last_hash`:

```rust
let html = render_oob_section(section, root_idx, mode, links);
let h = stable_hash(&html);
if last_hash[mode][root_idx] != Some(h) { ... }
```

The "the hash a subscriber seeds equals the hash the loop computes on its first tick for unchanged bytes" invariant is enforced today by careful authorship: both call sites must call `render_oob_section` with the same `(scan, root_idx, mode, links)` and `stable_hash` over the same bytes. There is nothing structural keeping them aligned. ADR-0024 (autosync section-level OOB swap, byte equality between rescan and SSE paths) is preserved by this same authorship discipline.

`service::render_view` is in a different category. It produces a `FlaggedView` (a `Vec<RootSection>`) for the page-render path: no OOB wrapping, no hashing. Folding it into the same iterator would force the page path to materialize OOB HTML and hashes it then discards, or push the work behind lazy combinators that obscure the simple `raw.iter().map(...)` shape it has today. It stays put.

## Goal

Make the "what counts as the wire bytes for a section, and what hash represents them" answer live in one place inside `autosync.rs`, so the snapshot/seed and loop/diff paths cannot drift.

## Change

Add one private helper to `src/autosync.rs`:

```rust
/// Render one section's OOB-swap bytes and hash them. Shared by
/// `snapshot_and_seed` and `compute_pushes` so the seed hash and the loop's
/// first-tick hash agree by construction (ADR-0024).
fn rendered_oob_with_hash(
    scan: &crate::scanner::RootScan,
    root_idx: usize,
    mode: ViewMode,
    links: &[crate::config::SearchLink],
) -> (String, u64) {
    let oob = render_oob_section(scan, root_idx, mode, links);
    let hash = stable_hash(&oob);
    (oob, hash)
}
```

Update the two call sites to consume the pair.

`snapshot_and_seed`:

```rust
for (root_idx, section) in raw.iter().enumerate() {
    let (oob, hash) = rendered_oob_with_hash(section, root_idx, mode, links);
    hashes.push(hash);
    payload.push_str(&oob);
}
```

`compute_pushes` inner loop:

```rust
for (root_idx, section) in raw.iter().enumerate() {
    let (html, h) = rendered_oob_with_hash(section, root_idx, mode, links);
    if last_hash[mode][root_idx] != Some(h) {
        last_hash[mode][root_idx] = Some(h);
        pushes.push((mode, root_idx, html));
    }
}
```

Nothing else changes. `render_oob_section` and `stable_hash` keep their current signatures and remain accessible to the existing tests that exercise them directly.

## Non-goals

- No change to `service::render_view` or the page-render path.
- No new public API on `RawView`. The original #7 framing of `RawView::sections(mode) -> impl Iterator<Item = RenderedSection>` is dropped: post-#3, the iteration shape on `RawView` is already a one-liner (`raw.iter().map(|s| render_section_from_raw(s, mode))`) and wrapping it would relocate rather than concentrate.
- No change to `render_oob_section`, `stable_hash`, or `single_oob_section`.
- No change to ADRs.

## Tests

Existing coverage stays:

- `render_oob_section_bytes_match_a_direct_single_oob_section_render` (autosync.rs:667) keeps verifying the OOB-wrapping boundary against `single_oob_section`.
- `render_oob_section_html_carries_total_audiobooks_for_a_walked_root` (autosync.rs:696) keeps verifying the per-root payload.
- All `compute_pushes` and `snapshot_and_seed` tests continue to pass unchanged: behavior is identical, only the inner two-line pair becomes a function call.

Add one test pinning the pair contract:

```rust
#[test]
fn rendered_oob_with_hash_returns_render_oob_section_paired_with_its_stable_hash() {
    // Mirror the walked-RootScan setup from
    // `render_oob_section_html_carries_total_audiobooks_for_a_walked_root`.
    let raw = /* representative walked RootScan */;
    let links: &[crate::config::SearchLink] = &[];

    let (oob, hash) = rendered_oob_with_hash(&raw, 0, ViewMode::GapsOnly, links);
    let direct_oob = render_oob_section(&raw, 0, ViewMode::GapsOnly, links);
    let direct_hash = stable_hash(&direct_oob);

    assert_eq!(oob, direct_oob);
    assert_eq!(hash, direct_hash);
}
```

## Deletion test

Removes one duplicated two-line `render_oob_section`/`stable_hash` pair from `compute_pushes` and `snapshot_and_seed`. Adds one ~6-line helper plus one test. Net source change is roughly neutral. The payoff is the structural invariant: the seed-hash/first-tick-hash agreement cannot drift because the two call sites share the function that produces both.

## ADR notes

ADR-0023 (autosync only runs while subscribed) and ADR-0024 (section-level OOB swap, byte equality) constrain what the loop does, not how the pair is computed. ADR-0024 is reinforced: the rendered bytes and the hash representing those bytes are now produced together by one function, so the snapshot path and the loop path provably hash the same string.

No new ADR is needed.

## Composition with the rest of architecture review

This closes the meaningful remaining duplication from candidate #7. The original #7 framing also called for "renderer generic on `compute_pushes` goes away," which already shipped in commit `63be7f3` ahead of this work. With this spec landed, the architecture-review README can mark #7 as done.
