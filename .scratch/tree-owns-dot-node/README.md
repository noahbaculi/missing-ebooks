# tree.rs owns the ADR-0005 `.`-node rule end-to-end

Architecture-review candidate #4 from `.scratch/architecture-review-2026-06/README.md`. Concentrates ADR-0005's two halves (loose-root becomes a pinned `.`-node, name comes from the canonical path's last component) into one module, so the renderer's `.`-node tests and the scanner's loose-root tests reach behavior through one function.

## Status

Implemented 2026-06-24 in commits `9e7e342` (move types) and `9976a63` (reshape `tree::build`).

## Problem

ADR-0005 says a library root can itself be a flagged folder when loose audio sits in it. The rule's two halves live apart:

- `src/tree.rs::build` (L67-108) knows that an empty `rel_path` `ScannedFolder` turns into a pinned node with `rel_path = "."` and a name supplied by the caller.
- `src/service.rs::render_root_state` (L194-209) derives that name by `canonical_path.file_name()` and threads it through as `root_name: &str`.

Renderer tests for the `.` node and scanner tests for the loose root assert via different paths. The name is not validated to actually match the root: three callers (production, demo handler, unit tests with `"Audiobooks"`) pass whatever they like.

The "pinned-first" invariant lives as an `roots.insert(0, ...)` call rather than a type-level field.

## After

`tree::build` takes the full `RootScan` and the `ViewMode`, returns a `RootState`. service.rs::render_root_state goes away; its one call site builds the `RootSection` directly. `RootState`, `ViewMode`, and `Node` all live in `tree.rs`, which becomes the canonical place for the per-root view model.

### Module layout

Conceptual stack from low to high: `scanner` -> `tree` -> `service` -> `web`.

- `tree.rs` gains: `RootState` and `ViewMode` (moved from `service.rs`), keeps `Node`.
- `service.rs` loses: `RootState`, `ViewMode`, and `render_root_state`. Imports `RootState` and `ViewMode` from `tree`.

`RootSection` and `FlaggedView` stay in `service.rs`. They are orchestration-layer aggregates, not per-root view shape.

### New `tree::build` signature

```rust
/// Builds the `RootState` for one library root in the requested mode.
///
/// Dispatches over the `RootScan` variant, derives the display name from the
/// canonical path for the loose-root `.`-node (ADR-0005), applies the gaps
/// filter when `mode` is `ViewMode::GapsOnly`, and collapses an empty result
/// to `RootState::Clean`.
pub fn build(scan: &RootScan, mode: ViewMode) -> RootState
```

Behavior by variant:

- `RootScan::Failed { message, .. }` returns `RootState::Error(message.clone())`.
- `RootScan::Walked { canonical_path, folders }` with empty `folders` returns `RootState::Clean`.
- `RootScan::Walked { canonical_path, folders }` otherwise: pick the working set (`reduce_to_flagged(folders)` for `GapsOnly`, the slice as-is for `All`), derive the display name from `canonical_path.file_name()` with `"."` as the fallback, build the forest, and return `RootState::Clean` if the forest is empty or `RootState::Forest(forest)` otherwise.

The private forest-construction path (`insert_all`, `sort_forest`, the `.`-node pinning) does not change. Only the public seam moves.

### service.rs collapse

`render_root_state` disappears. Its one call site in the per-section pipeline becomes:

```rust
RootSection {
    path: scan.display_path(),
    state: tree::build(scan, mode),
    total_audiobooks: scan.audiobook_count(),
}
```

The `root_name` extraction at service.rs:206-209 (`canonical_path.file_name().and_then(...).unwrap_or(".")`) is gone, now folded into `tree::build`.

### Test migration

Six in-file tests in `tree.rs::tests` call `build("Audiobooks", &folders)` today. They migrate to constructing a `RootScan::Walked` and choosing a mode. A small test helper keeps the call sites readable:

```rust
fn walked(name: &str, folders: Vec<ScannedFolder>) -> RootScan {
    RootScan::Walked {
        canonical_path: PathBuf::from("/lib").join(name),
        folders,
    }
}
```

A typical test body becomes:

```rust
let scan = walked("Audiobooks", folders);
let state = build(&scan, ViewMode::All);
let RootState::Forest(forest) = state else { panic!("expected forest") };
```

The four existing `build_all_*` tests stay on `ViewMode::All`. The `build_carries_audio_files_onto_a_flagged_leaf` test pre-filters folders before calling `build` today; it can either stay on `ViewMode::All` with a pre-filtered scan, or switch to `ViewMode::GapsOnly` with an unfiltered scan. Pick whichever reads cleaner per test; both exercise the same code path.

service.rs tests that match against `RootState::Forest(_) | RootState::Clean | RootState::Error(_)` keep working: update the `use` to point at `crate::tree` instead of `crate::service`.

## ADR notes

- ADR-0005 (library root itself flaggable): the rule, not the shape. Compatible. This change concentrates the rule, which is the point of the candidate.
- ADR-0024 (autosync section-level OOB swap): byte equality between SSE and rescan paths is preserved because both paths reach the renderer through the same `tree::build`.

No new ADR is needed: ADR-0005 already records the rule, and the module placement is mechanical from it.

## Deletion test

Passes.

Removed:

- `service.rs::render_root_state` (~30 lines including doc)
- `service.rs::RootState` definition (~10 lines with serde attrs)
- `service.rs::ViewMode` definition
- the `root_name` derivation at the one call site

Added:

- `RootState` and `ViewMode` moved into `tree.rs` (same line count, new home)
- `tree::build`'s `RootScan` dispatch, name derivation, and mode filter (~15 lines)

Net line count: roughly even. The win is locality. The `.`-node convention, the name extraction, the gaps filter, and the empty -> Clean collapse all sit in one module.

## Out of scope

- No change to the renderer (`src/web/render.rs`): it already iterates `RootState::Forest(nodes)` uniformly and does not special-case the `.` node.
- No change to `RootSection` or `FlaggedView`.
- No change to the demo handler: it builds `RootSection` values directly and does not call `tree::build`.
- No change to `tree::Node`'s shape.

## Composition with other candidates

Independent of the remaining open candidates (#5 declarative `ScenarioSpec`, #6 `Renderer` context). Builds on #1 (`scanner::scan_root` owning canonicalize + classify + `RootScan`): the `canonical_path` field on `RootScan::Walked` is what makes the name derivation a one-liner inside `tree::build`.
