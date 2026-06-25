# Fold `service.rs` into web handlers and renderer

Architecture review Candidate 1, from `.scratch/architecture-review/findings.md`.

## Why

After ADR-0027 consolidated the scan substrate and the marker IO behind `RawViewStore`, every operation in `src/service.rs` collapsed to a three-line shape: take a result off the store, package it for the renderer, optionally wrap it in `Arc`. The module's stated purpose (a web-agnostic service layer shared by the HTML UI and a future JSON API) survives by hypothetical, not by use. The only consumer of the four async wrappers is `src/web.rs`. The findings doc calls this out as a shallow module: the public interface (four async fns plus four types) is nearly as wide as the production implementation (~110 lines under ~530 lines of tests).

Folding the wrappers into the four handlers concentrates complexity in the modules that are already deep (`RawViewStore` and `web::render`) and drops a module hop per request. The two helpers that earn their keep (the raw-to-packaged renderer and its per-section twin) move next to the markup they feed. The error type moves next to the store that constructs it. The packaging type that exists for one handler dissolves into that handler.

This is the fold-away direction the findings doc recommends. The alternative (growing `service.rs` to own the `ViewMode -> RawView -> FlaggedView -> Markup` pipeline) crosses ADR-0027's "revisit if a future API surface needs raw scan output" tripwire without that API actually being on the horizon.

## End state

`src/service.rs` is deleted. Its surface relocates as follows:

| Symbol today | Lives where after | Visibility |
| --- | --- | --- |
| `service::current_view` | inlined into `web::index` and the `main.rs` warm-up | gone |
| `service::rescan` | inlined into `web::rescan` | gone |
| `service::mark` | inlined into `web::mark` | gone |
| `service::unmark` | inlined into `web::unmark` | gone |
| `service::MarkOutcome` | dissolved; handler reads `Applied.created` off the store result | gone |
| `service::render_view` | renamed to `web::render::package_view` | `pub(crate)` |
| `service::render_section_from_raw` | renamed to `web::render::package_section` | `pub(crate)` |
| `service::FlaggedView` | `web::render::FlaggedView` | `pub` (demo consumes) |
| `service::RootSection` | `web::render::RootSection` | `pub` (demo consumes) |
| `service::DomainError` | `state::DomainError` | `pub` (web matches on) |

`Arc<FlaggedView>` does not appear in the new code paths. Handlers hold `FlaggedView` by value, borrow one `&RootSection` for the section-shaped responses, and drop the view at the end of the handler. No production caller holds a `FlaggedView` across an `await` after a handler returns it.

After the fold, the read-side request paths are:

- `web::index`: `store.current().await` -> `package_view(&raw, mode)` -> `render::render_view(&view, links, mode).into_string()`.
- `web::rescan`: `store.rescan().await` -> `package_view(&raw, mode)` -> `render::roots(&view, links, mode)` wrapped with `HX-Push-Url`.
- `web::mark`: `store.write_mark(...)` returns `Result<Applied, DomainError>`. On `Ok`, `package_view(&applied.raw, mode)`, render the affected section, build a `marked_trigger` iff `applied.created`. On `Err`, `failed_write_response`.
- `web::unmark`: `store.remove_mark(...)` returns `Result<Arc<RawView>, DomainError>`. On `Ok`, `package_view(&raw, mode)`, render the affected section. On `Err`, `failed_write_response`. The `Arc` is the store's own (it hands out a clone of the slot); the handler dereferences it for packaging and drops it at the end of the block.
- `web::failed_write_response`: unchanged behavior. Calls `store.current().await`, packages, renders the section. Candidate 5's elimination of this re-fetch is explicitly out of scope.

## Helper rename rationale

`web/render.rs` already owns a `pub(crate) fn render_view(view: &FlaggedView, links, mode) -> Markup`. Moving `service::render_view` (which builds the `FlaggedView`, not the markup) into the same module would collide. The raw-to-packaged step is packaging, not rendering: it builds the renderer's input. So:

- `service::render_view` -> `web::render::package_view`
- `service::render_section_from_raw` -> `web::render::package_section`

Bare token names, terse, no overlap with the markup-producing pair. The doc comments on both helpers stay verb-first and explain the same thing they explain today (one runs the gaps filter and forest build per section; both are colocated with the per-section state derivation).

## Type and import movements

`web/render.rs` adds `pub type FlaggedView = Vec<RootSection>` and `pub struct RootSection { path, state, total_audiobooks }`, deriving the same set as today (`Debug, Clone, PartialEq, Eq, Serialize`). It drops `use crate::service::{FlaggedView, RootSection};`. It gains `use crate::state::RawView` and `use crate::scanner::RootScan` to support the new helpers.

`state.rs` adds `DomainError` (verbatim move: same variants, same `#[error(...)]` attributes, same `#[derive(Debug, Error)]`). It drops `use crate::service::DomainError;`. Every internal constructor (`write_marker`, `delete_marker`, `write_mark`, `remove_mark`) keeps building it through unprefixed local paths.

`autosync.rs` rewrites the one import line: `crate::service::render_section_from_raw` becomes `crate::web::render::package_section`. The `render_oob_section` function body changes only the call name. The two doc comments mentioning `service::render_section_from_raw` get the new path.

`demo/handlers.rs` rewrites `use crate::service::{FlaggedView, render_view};` to `use crate::web::render::{FlaggedView, package_view};`. The `derive_view` function calls `package_view` instead of `render_view`.

`main.rs` warm-up replaces:

```rust
let _ = missing_ebooks::service::current_view(
    &state,
    missing_ebooks::tree::ViewMode::GapsOnly,
)
.await;
```

with:

```rust
// Warm the gaps-mode slot. The packaging is cheap; the side effect on the
// cache slot is what we want.
let _ = state.store.current().await;
```

`web/page.rs` keeps its existing module doc comment reference to `FlaggedView`; the type is still inside `web::`, so the prose is still accurate.

`lib.rs` loses `pub mod service;`.

## Test relocation

The 27 tests under `service::tests` sort as follows. The order of receiving sections is `state::tests`, `web/render.rs::tests`, `tree::tests`, `scanner::tests`. Each receiving module already has a tests block.

**Store behavior -> `state::tests`** (15 tests, lines reference today's `src/service.rs`):

- `cache_hit_within_ttl_serves_the_same_raw_slot` (284)
- `warm_concurrent_reads_share_one_raw_slot_and_render_equally` (303)
- `ttl_zero_rescans_every_call` (331)
- `rescan_refreshes_even_within_a_live_ttl` (355)
- `rescan_clears_the_dir_index_then_repopulates_it` (369)
- `a_warm_state_rescan_reuses_the_index` (428)
- `mark_invalidates_the_marked_dir_in_the_index` (445)
- `mark_updates_a_warm_cache_in_place_without_rescanning` (491)
- `mark_on_a_cold_cache_scans_fresh` (522)
- `mark_outside_a_root_is_rejected` (541)
- `mark_with_a_bad_root_index_errors` (552)
- `unmark_deletes_the_file_and_re_flags_the_root` (562)
- `unmark_with_a_bad_root_index_errors` (590)

Plus the `state_for` test helper (277) merges into `state::tests`' existing setup. Several of these have close cousins already in `state::tests`; the implementation plan decides per test whether to add, merge, or drop as duplicate.

**Render packaging behavior -> `web/render.rs::tests`** (5 tests):

- `root_with_a_gap_yields_a_matching_forest` (163)
- `root_with_no_audio_is_clean` (180)
- `missing_root_is_error_and_other_roots_still_render` (190)
- `render_view_computes_total_audiobooks_per_root` (251)
- `all_mode_builds_the_full_tree_including_covered_folders` (622)

These keep their integration shape (real tempdir + real scan) because packaging is what they exercise; synthetic `FlaggedView` fixtures would test the wrong layer.

**Type-shape -> `web/render.rs::tests`** (1 test):

- `root_states_serialize_to_stable_json` (470) goes next to `RootSection`.

**`RootScan` shape -> `scanner.rs::tests`** (1 test):

- `audiobook_count_counts_walked_folders_that_directly_hold_audio` (206) is a pure `RootScan::audiobook_count` test; it belongs with the type.

**`ViewMode` -> `tree.rs::tests`** (3 tests):

- `view_mode_parses_the_query_token_leniently` (599)
- `view_mode_round_trips_through_its_query_token` (608)
- `view_mode_path_returns_canonical_url_per_mode` (615)

**Test helpers**: `test_config`, `test_settings`, `test_index` exist in `state::tests` already; the migration reuses those rather than carrying duplicates over.

After all 27 tests move, the `mod tests` block in `service.rs` is empty and the file is deleted (see commit plan).

## ADR-0028

Add `docs/adr/0028-service-layer-folded-into-handlers-and-renderer.md` recording the deletion of the service layer. Same prose-paragraph style as ADR-0027 (no header ceremony, a single `## Related` footer). The ADR covers:

- What the layer was: four wrappers, two helpers, four types, with ADR-0027's "revisit if a JSON API arrives" clause as the previous decision point.
- What it is now: wrappers inlined into handlers; helpers and `FlaggedView`/`RootSection` next to the renderer; `DomainError` next to the store; `MarkOutcome` and `Arc<FlaggedView>` deleted.
- Rejected alternatives: growing `service.rs` (no JSON API on the horizon, so the tripwire is not earned); keeping the wrappers as thin pass-throughs (paid the indirection cost for nothing).
- Revisit if: a second HTTP-shaped consumer appears. At that point the shared response shape (package raw -> render section -> attach HX headers and triggers) is worth lifting back out into a `response` module, not a generic service layer.
- Related: ADR-0002 (marker-write invariant; preserved inside `state.rs`), ADR-0022 (cache holds raw; preserved), ADR-0027 (substrate consolidation; this ADR extends it).

## Commit plan

Granular, conventional, no squash. Build green at every commit.

The ordering rule is: relocate types and tests before deleting the wrappers they depend on. The 27 tests under `service::tests` call `current_view`, `rescan`, `mark`, and `unmark` directly (see lines 289-593 of today's `service.rs`); deleting those wrappers before moving the tests would break the build mid-plan. So test moves happen first, handler folds after.

1. `docs: record service-layer fold as ADR-0028` (the ADR file only).
2. `refactor(state): move DomainError to state.rs` (enum moves; `service.rs` keeps a `pub use crate::state::DomainError;` stub so the rest of the commits compile).
3. `refactor(render): move FlaggedView, RootSection, package_view, package_section to web/render.rs` (types and helpers move; `service.rs` re-exports them under both old and new names; `web/render.rs`'s `use crate::service::{...}` line is deleted; `service.rs::tests` keeps compiling via `super::*` pulling in the re-exports).
4. `chore(tests): move store-behavior tests from service to state::tests` (the 13 store-behavior tests; each is compared against the existing `state::tests` block first, then added, merged into a near-duplicate, or dropped outright; rewritten to call `store.X()` directly rather than going through the soon-to-be-deleted wrappers; the `state_for` helper is dropped since `state::tests::test_store` covers the same need).
5. `chore(tests): move render-packaging and type-shape tests from service to render::tests` (the 5 packaging tests plus `root_states_serialize_to_stable_json`; tests call `package_view` / `package_section` directly).
6. `chore(tests): move ViewMode tests to tree::tests and audiobook_count test to scanner::tests` (the 3 `ViewMode` tests plus `audiobook_count_counts_walked_folders_that_directly_hold_audio`).
7. `refactor(web): inline service::current_view into web::index and main warm-up` (handler and `main.rs` warm-up fold; `service::current_view` deleted).
8. `refactor(web): inline service::rescan into web::rescan` (handler fold; `service::rescan` deleted).
9. `refactor(web): inline service::mark into web::mark; drop MarkOutcome` (handler fold; `MarkOutcome` deleted; `Arc::new` wrappers vanish on this path).
10. `refactor(web): inline service::unmark into web::unmark` (handler fold; `service::unmark` deleted; `Arc::new` wrapper vanishes on this path).
11. `refactor(autosync): rewire render_section_from_raw to web::render::package_section` (one-line import and call-site rename).
12. `refactor(demo): rewire render_view to web::render::package_view` (one-line import and call-site rename).
13. `chore: delete src/service.rs` (file disappears; `lib.rs` loses `pub mod service;`; the re-export stubs in `service.rs` go with it; the `pub use` lines that other modules may still carry are removed in this commit).

`cargo test` and the pre-commit hook (fmt, clippy, doc -D warnings, accent test when assets or accent tests change) run on every commit.

## Out of scope

- Candidate 5 (`failed_write_response` re-fetch elimination). The findings doc says it composes with this work, but rolling it in widens the blast radius. Separate spec.
- Candidate 3 (demo seam closure around `apply_mark_raw` / `build_view`). Separate spec.
- Any change to `package_view`'s body, the store's interface, the markup, or the tracing fields. The fold is mechanical.
- Renaming `MarkRequest`, `ViewQuery`, or any handler-only type.

## Non-goals

- No new public surface. `package_view` and `package_section` are `pub(crate)`. `FlaggedView`, `RootSection`, `DomainError` stay `pub` because demo and web handlers already need them.
- No behavior change. The only observable shifts are one fewer module hop per request and one fewer `Arc<FlaggedView>` allocation per index/mark/unmark/rescan.
- No new dependencies, no new modules.

## Files touched

- Modify: `src/web.rs` (handlers absorb the wrappers), `src/web/render.rs` (gains types and packaging helpers), `src/state.rs` (gains `DomainError`), `src/autosync.rs` (rewires one call site), `src/demo/handlers.rs` (rewires one call site), `src/main.rs` (warm-up), `src/lib.rs` (drops `pub mod service`), `src/scanner.rs::tests` (gains one test), `src/tree.rs::tests` (gains three tests).
- Delete: `src/service.rs`.
- Add: `docs/adr/0028-service-layer-folded-into-handlers-and-renderer.md`.

## Constraints

- Comments follow the `writing-style-code-comments` style: verb-first or noun-phrase doc summaries, terse, no "This function" openers, no em dashes, backticks around identifiers and literals.
- Prose in this spec, the ADR, and the implementation plan follows the `humanizer` skill. No em dashes anywhere.
- Conventional Commits (`type(scope): subject`), no squash, no `--no-verify`.
- After each commit: `cargo test` must pass. Pre-commit hook runs fmt, clippy, `cargo doc -D warnings`, and the accent test for asset or accent changes.
