# ADR-0033: raw-view type and rule split out of state.rs

> Amended 2026-07-06 by ADR-0034: SSE autosync is removed. The `autosync` module named as a raw-view consumer in the Context section is gone; `web::render` and `demo::overlay` remain.

Date: 2026-07-05.

## Context

ADR-0027 consolidated the substrate (config, scan settings, dir index, cache slot, marker file IO) behind `RawViewStore` in `src/state.rs`. It left the raw-view type, the pure marker-apply rule, and the raw-view constructor colocated with the store in the same file. Callers that only need raw-view semantics (`demo::overlay` as its semantic oracle, `web::render` as its consumer, `autosync` on the SSE path, `demo::state` for the demo's static base) pulled the whole store module for the type. At 1322 lines, `state.rs` told two stories: the raw view and the store around it.

## Decision

`src/raw_view.rs` owns the raw-view type (`RawView`), the pure marker-apply rule (`apply_mark_raw` plus its `add_marker` helper), and the async constructor (`build_view` plus its `build_section` helper). `src/state.rs` owns the store, its cache slot, its coalescer, its lock discipline, and the marker file IO (`write_marker`, `delete_marker`, `Applied`, `WriteError`, `WriteFailure`). Every consumer imports from `crate::raw_view::…` directly; `state` does not re-export the moved items. `build_section` widens from module-private to `pub(crate)` so `RawViewStore::remove_mark` can still call it after the move; every other signature is unchanged.

## Consequences

`state.rs`'s substrate role becomes its only story. `demo::overlay` reaches `apply_mark_raw` (its semantic oracle) without pulling the store module. `demo::state` builds its static base view against the raw-view module directly. The move unblocks candidate #1 of the 2026-07-05 architecture review (one `ViewStore` interface with two adapters) without adding a new cross-module dependency arrow: a future file-backed prod store and a session-overlay store would each depend on `raw_view` for the type and on `state` for `RawViewStore`, not on each other.

ADR-0027 stays intact. The store still owns cache slot plus marker IO together. This ADR moves only the raw-view type and the pure rule (plus the async constructor that produces one). No IO moves. ADR-0027's revisit clause (split cache from IO if a JSON or CLI consumer arrives without needing marker IO) is unchanged.

Revisit if a future consumer needs `apply_mark_raw` outside the crate. The rule is `pub(crate)` today; publishing it means adding unit tests beyond the byte-equality gate that currently pins it.

## Related

- ADR-0027 (substrate consolidated behind `RawViewStore`): preserved. The store still owns cache slot plus marker IO in one type.
- ADR-0002 (marker writes edit cache in place): preserved. The invariant still lives inside `RawViewStore::write_mark`, which now calls `raw_view::apply_mark_raw`.
- ADR-0005 (library root itself flaggable): preserved. The `rel == "."` root-mark case moves with `apply_mark_raw`.
- ADR-0022 (cache holds raw scan output): preserved. Per-request rendering shape unchanged.
