# Raw-view module closes the demo seam

Date: 2026-06-25.

## Context

`src/state.rs` exported three items past its interface purely for the demo's benefit: `pub type RawView = Vec<RootScan>`, `pub(crate) fn apply_mark_raw`, and `pub(crate) async fn build_view`. `RawViewStore`'s public surface is its four async methods (`current`, `refresh`, `rescan`, `write_mark`, `remove_mark`), each of which produces a refreshed view atomically. That shape fits the production handler ("write a mark, get back the refreshed view") but not the demo ("apply N session marks to a shared base, never mutate it"), so `src/demo/handlers.rs` and `src/demo/state.rs` reached around the store and grabbed the in-place edit primitive (`apply_mark_raw`) and the pure scan invoke (`build_view`) directly. Candidate 3 of the 2026-06 architecture review flagged this as seam leakage: three internals exposed to support one consumer, and the consumer reimplementing a per-session derivation in `demo::handlers::derive_view` rather than asking the store for one.

## Decision

A new peer module `src/raw_view.rs` now owns `RawView`, `apply_mark_raw`, `build_view`, and the `build_section` helper. Both `state.rs` and `demo/` import from `raw_view` as equals. `RawViewStore`'s interface is unchanged: `write_mark` still applies the mark in place under the cache lock by calling `raw_view::apply_mark_raw`, and `build_view` still drives the per-root scans the cache memoizes. The demo's `derive_view` still clones the shared base and replays its marks in a loop, now importing the primitive from `crate::raw_view` rather than `crate::state`. The `pub(crate)` visibility on `apply_mark_raw` and `build_view` widens to `pub` in the new module because the demo (in the same binary crate) is no longer a privileged peer of `state.rs`; nothing outside the crate links against either path.

## Consequences

Alternatives we set aside. Folding the items into `src/scanner.rs` would put an in-memory edit primitive next to disk-walking code with no shared concern; the type alias `Vec<RootScan>` and `RootScan` belong in the same module by coincidence of type, not by shared role. Splitting `state.rs` into `state/mod.rs` plus `state/raw_view.rs` would keep the demo importing from `crate::state`, which is the seam this ADR closes. Growing `RawViewStore` with a per-session `derive(base, marks) -> Arc<RawView>` method would not help the demo at all, since the demo holds no `RawViewStore`.

Revisit if the demo goes away, in which case the four items collapse back into `state.rs` and `apply_mark_raw` plus `build_view` return to `pub(crate)`. Also revisit if a third consumer of `apply_mark_raw` appears whose shape fits neither the store's lock-held single-mark path nor the demo's clone-then-loop; at that point a named `derive_with_marks(base, marks) -> RawView` primitive earns its keep.

## Related

- ADR-0002: marker writes edit cache in place. Preserved; the invariant still lives inside `RawViewStore::write_mark`, which now calls `raw_view::apply_mark_raw`.
- ADR-0022: cache holds raw scan output. Preserved; the store still holds raw views and the renderer still packages per `ViewMode` on each read.
- ADR-0027: substrate consolidated behind `RawViewStore`. Preserved; the store still owns the cache slot, the substrate Arcs, and the marker file IO.
- ADR-0028: service layer folded into handlers and renderer. Preserved; this ADR extends the same compaction one module further by closing the demo's reach into `state.rs`.
