# Fold `service.rs` into web handlers and renderer: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete `src/service.rs`. Inline the four async wrappers (`current_view`, `rescan`, `mark`, `unmark`) into their four web handlers. Move `FlaggedView`, `RootSection`, and the two render helpers (renamed to `package_view` and `package_section`) to `src/web/render.rs`. Move `DomainError` to `src/state.rs` next to the store that constructs it. Drop the `Arc<FlaggedView>` wrappers and the `MarkOutcome` type.

**Architecture:** Mechanical migration. No behavior change. The deep modules (`RawViewStore` in `state.rs`, `render::*` in `web/render.rs`) stay as-is; the wrapper layer between them and the handlers dissolves. Test moves precede handler folds so the build stays green at every commit: the existing `service::tests` block calls the wrappers directly across roughly 25 sites, so the wrappers must outlive the tests that depend on them.

**Tech Stack:** Rust 2024 edition, `axum`, `maud`, `tokio`, `serde`, `thiserror`. Test framework: `cargo test`. Pre-commit hook: `cargo fmt`, `cargo clippy`, `cargo doc -D warnings`, plus the accent test for asset and accent changes.

## Global constraints

- Code-comments style (`writing-style-code-comments`): terse, verb-first or noun-phrase doc summaries, no em dashes, no "This function" openers, backticks around identifiers and literals. Inline comments are imperatives or bare label noun phrases.
- No em dashes in any prose or comment, ever (`AGENTS.md`).
- Commits follow Conventional Commits (`type(scope): subject`). Granular, no squashing.
- Pre-commit hook runs `cargo fmt`, `cargo clippy`, `cargo doc -D warnings`. Never bypass with `--no-verify`.
- After every task: `cargo test` must pass.
- Spec: `.scratch/candidate-1-fold-service/spec.md`. Read it before starting any task whose details look ambiguous.

## File map

- Modify: `src/state.rs` (gains `DomainError`, gains the ported store-behavior tests).
- Modify: `src/web/render.rs` (gains `FlaggedView`, `RootSection`, `package_view`, `package_section`; gains the ported render-packaging tests).
- Modify: `src/web.rs` (handlers absorb the wrappers; `failed_write_response` rewrites).
- Modify: `src/autosync.rs` (one import and one call site rewire; two doc-comment mentions update).
- Modify: `src/demo/handlers.rs` (one import and one call site rewire).
- Modify: `src/main.rs` (warm-up loses the `service::` call).
- Modify: `src/lib.rs` (drops `pub mod service;`).
- Modify: `src/tree.rs` (gains three `ViewMode` tests).
- Modify: `src/scanner.rs` (gains one `audiobook_count` test).
- Delete: `src/service.rs`.
- Add: `docs/adr/0028-service-layer-folded-into-handlers-and-renderer.md`.

---

## Task 1: Add ADR-0028

Record the architectural decision. The ADR captures what the service layer was, what replaces it, the alternatives we rejected, and the revisit-if clause. Lands first so every later commit can cite ADR-0028 in its body.

**Files:**
- Create: `docs/adr/0028-service-layer-folded-into-handlers-and-renderer.md`

- [ ] **Step 1: Read ADR-0027 to match its prose-paragraph style**

  Open `docs/adr/0027-substrate-consolidated-behind-rawviewstore.md`. The shape is: title, four prose paragraphs (what was, what is, alternatives, revisit-if), then a `## Related` footer. No section headers inside the body. Use that shape verbatim for ADR-0028.

- [ ] **Step 2: Write the ADR**

  Create `docs/adr/0028-service-layer-folded-into-handlers-and-renderer.md` with the following content:

  ```markdown
  # Service layer folded into handlers and renderer

  `src/service.rs` carried four async wrappers (`current_view`, `rescan`, `mark`, `unmark`), two render helpers (`render_view`, `render_section_from_raw`), and four types (`FlaggedView`, `RootSection`, `DomainError`, `MarkOutcome`). Its stated purpose was a web-agnostic service layer shared by the HTML UI and a future JSON API. After ADR-0027 consolidated the scan substrate and the marker IO behind `RawViewStore`, each wrapper collapsed to a three-line shape: take a result off the store, render it for the view mode, optionally wrap it in `Arc`. The only consumer of the four async wrappers was `src/web.rs`. The architecture review in `.scratch/architecture-review/findings.md` (Candidate 1) flagged the module as shallow: a public interface (four async fns plus four types) nearly as wide as the production implementation under it.

  The four wrappers now inline into their four handlers in `src/web.rs`. `FlaggedView` and `RootSection` move to `src/web/render.rs` next to the markup that consumes them. The raw-to-packaged helpers move there too under terser names (`package_view` and `package_section`) so they do not collide with the existing markup-producing `render_view`. `DomainError` moves to `src/state.rs` next to the store that constructs it inside `write_marker` and `delete_marker`. `MarkOutcome` dissolves; the `web::mark` handler reads `Applied.created` directly off the store result. The `Arc<FlaggedView>` wrappers vanish; handlers hold `FlaggedView` by value, borrow one `&RootSection` for the section-shaped responses, and drop the view at the end of the response.

  Alternatives we set aside. Growing `service.rs` to own the `ViewMode -> RawView -> FlaggedView -> Markup` pipeline (the other direction from the findings doc) would have crossed ADR-0027's "revisit if a future API surface needs raw scan output" tripwire without that API actually being on the horizon. Keeping the wrappers as thin pass-throughs would have kept a module hop per request, an extra `Arc::new` per response, and a misplaced home for `DomainError` without paying for any of it.

  Revisit if a second HTTP-shaped consumer (JSON API, CLI HTTP harness) appears. At that point the shared response shape (package the raw view, render the section, attach `HX-*` headers and triggers) would be worth lifting back out, into a `response` module that owns the response packaging rather than a generic "service" layer.

  ## Related

  - ADR-0002: marker writes edit cache in place. Preserved; the invariant still lives inside `RawViewStore::write_mark`.
  - ADR-0022: cache holds raw scan output. Preserved; the store still holds raw.
  - ADR-0027: substrate consolidated behind `RawViewStore`. This ADR extends the consolidation by removing the thin layer above the store.
  ```

- [ ] **Step 3: Verify the ADR builds**

  Run:

  ```bash
  cargo doc -D warnings
  ```

  Expected: success. Markdown files do not feed `cargo doc`, but the pre-commit hook runs this and we want the same gate.

- [ ] **Step 4: Commit**

  ```bash
  git add docs/adr/0028-service-layer-folded-into-handlers-and-renderer.md
  git commit -m "docs: record service-layer fold as ADR-0028

  Document the deletion of src/service.rs: the four async wrappers inline
  into their web handlers, FlaggedView/RootSection plus the renamed
  package_view/package_section helpers move to web/render.rs, DomainError
  moves next to RawViewStore in state.rs, and MarkOutcome plus the
  Arc<FlaggedView> wrappers dissolve. Extends ADR-0027 by removing the
  thin layer the substrate consolidation left above the store."
  ```

---

## Task 2: Move `DomainError` to `state.rs`

`DomainError` is defined in `service.rs` but every constructor lives in `state.rs` (`write_marker`, `delete_marker`, `write_mark`, `remove_mark`). The move puts the enum next to the IO that builds it. `service.rs` keeps a `pub use crate::state::DomainError;` re-export so the rest of the migration commits compile without changing import paths everywhere at once.

**Files:**
- Modify: `src/state.rs` (gains the enum; drops the import)
- Modify: `src/service.rs` (loses the enum; gains a re-export stub)

**Interfaces produced:**
- `pub enum state::DomainError` (variants: `RootIndex`, `OutsideRoots`, `TargetMissing`, `NotADirectory`, `WriteFailed(std::io::Error)`)
- `pub use crate::state::DomainError;` from `service.rs`

- [ ] **Step 1: Read the current `DomainError` in `service.rs`**

  Open `src/service.rs:35-52`. The enum has five variants with `#[error(...)]` attributes and `#[derive(Debug, Error)]`. Copy it verbatim; the doc comment on the enum and on each variant stays as-is.

- [ ] **Step 2: Add `DomainError` to `state.rs`**

  In `src/state.rs`, add the `thiserror::Error` import if not already pulled in (it is not in `state.rs` today). Insert the `DomainError` enum after the `Applied` struct (around line 191), before `impl RawViewStore`:

  ```rust
  /// A failure performing a write action. The HTML surface renders it inline. A
  /// future JSON API would render it as an error body.
  #[derive(Debug, thiserror::Error)]
  pub enum DomainError {
      /// The submitted root index does not name a configured root.
      #[error("no such library root")]
      RootIndex,
      /// The resolved target sits outside every configured root.
      #[error("target is outside the configured library roots")]
      OutsideRoots,
      /// The target folder does not exist, or could not be canonicalized.
      #[error("target folder does not exist")]
      TargetMissing,
      /// The target resolved to a file rather than a directory.
      #[error("target is not a directory")]
      NotADirectory,
      /// The marker file could not be written.
      #[error("could not write the marker file: {0}")]
      WriteFailed(std::io::Error),
  }
  ```

  Remove the `use crate::service::DomainError;` line near the top of `state.rs` (line 16). The enum is now local.

- [ ] **Step 3: Replace the `DomainError` definition in `service.rs` with a re-export**

  In `src/service.rs`, delete lines 33-52 (the `pub enum DomainError { ... }` block and its doc comment). Drop the `use thiserror::Error;` line at the top if it has no other consumer (it does not). Insert a re-export near the top, after the other `use` statements:

  ```rust
  pub use crate::state::DomainError;
  ```

- [ ] **Step 4: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count as before. `state.rs` constructors keep building `DomainError` through local resolution; `service.rs` tests resolve `DomainError` through the `pub use`; `web.rs` keeps matching on `service::DomainError` via the re-export.

- [ ] **Step 5: Commit**

  ```bash
  git add src/state.rs src/service.rs
  git commit -m "refactor(state): move DomainError to state.rs

  Move the DomainError enum from service.rs to state.rs. Every constructor
  (write_marker, delete_marker, write_mark, remove_mark) already lives in
  state.rs; the type now lives with them. service.rs keeps a pub use
  re-export so callers in web.rs, autosync.rs, and the test modules
  compile unchanged through the rest of the fold migration. The stub
  goes away with service.rs itself in the final commit."
  ```

---

## Task 3: Move types and render helpers to `web/render.rs`

`FlaggedView`, `RootSection`, `render_view`, and `render_section_from_raw` move to `web/render.rs`. The two helpers get terser names that do not collide with the existing markup-producing `render_view` in that module: `render_view` becomes `package_view`, `render_section_from_raw` becomes `package_section`. `service.rs` re-exports them under the new names plus the old names, so the tests inside `service::tests` and the consumers in `autosync.rs` and `demo/handlers.rs` keep compiling. `web/render.rs` drops its `use crate::service::{...}` line since the types are now local.

**Files:**
- Modify: `src/web/render.rs` (gains the types and helpers; drops the import)
- Modify: `src/service.rs` (loses the symbols; gains re-export stubs)

**Interfaces produced:**
- `pub type web::render::FlaggedView = Vec<RootSection>;`
- `pub struct web::render::RootSection { path: String, state: RootState, total_audiobooks: usize }`
- `pub(crate) fn web::render::package_view(raw: &state::RawView, mode: ViewMode) -> FlaggedView`
- `pub(crate) fn web::render::package_section(scan: &scanner::RootScan, mode: ViewMode) -> RootSection`
- Re-exports from `service.rs`:
  - `pub use crate::web::render::{FlaggedView, RootSection};`
  - `pub(crate) use crate::web::render::package_view as render_view;`
  - `pub(crate) use crate::web::render::package_section as render_section_from_raw;`

- [ ] **Step 1: Read the current definitions in `service.rs`**

  Open `src/service.rs:17-31` for `FlaggedView` and `RootSection`. Open `src/service.rs:111-133` for `render_view` and `render_section_from_raw`. Copy them verbatim for Step 2; the only change is the helper names.

- [ ] **Step 2: Insert the new symbols at the top of `web/render.rs`**

  Open `src/web/render.rs`. Add the missing imports near the existing `use` block (around line 11):

  ```rust
  use serde::Serialize;

  use crate::scanner::RootScan;
  use crate::state::RawView;
  use crate::tree;
  ```

  (`crate::tree::{RootState, ViewMode, Node}` is already imported; the bare `crate::tree` is for the `tree::build` call inside `package_section`.)

  Insert the types and helpers immediately after the imports, before `fn chevron()`:

  ```rust
  /// The whole read view: one section per configured library root, in config order.
  pub type FlaggedView = Vec<RootSection>;

  /// One library root's outcome, labeled with the path the scanner walked.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
  pub struct RootSection {
      /// The canonical root path when it resolved, else the configured path.
      pub path: String,
      /// What the scan found for this root.
      pub state: RootState,
      /// Folders under this root that directly hold audio. Zero for `Clean` and
      /// `Error`. The web layer surfaces it as `data-total-audiobooks` on the
      /// section so the strip's library coverage stays current across swaps.
      pub total_audiobooks: usize,
  }

  /// Build the per-mode `FlaggedView` from the cached raw scan output. The gaps
  /// path filters with `reduce_to_flagged` and builds the forest. Show-all builds
  /// directly from the raw folders. Both run on the request thread (the per-folder
  /// cost is bounded, see ADR-0022). Allocates a fresh `FlaggedView` per response
  /// and drops it after the response writes.
  pub(crate) fn package_view(raw: &RawView, mode: ViewMode) -> FlaggedView {
      raw.iter()
          .map(|scan| package_section(scan, mode))
          .collect()
  }

  /// Build one `RootSection` from a raw `RootScan` for the requested mode.
  ///
  /// The single owner of the raw-to-packaged step. `package_view` calls it on
  /// the snapshot path; `autosync::render_oob_section` calls it on the push
  /// path. Any future per-root field lands here once.
  pub(crate) fn package_section(scan: &RootScan, mode: ViewMode) -> RootSection {
      RootSection {
          path: scan.display_path().to_string(),
          state: tree::build(scan, mode),
          total_audiobooks: scan.audiobook_count(),
      }
  }
  ```

  Delete the `use crate::service::{FlaggedView, RootSection};` line (line 9) since the types are now local.

- [ ] **Step 3: Replace the moved symbols in `service.rs` with re-exports**

  In `src/service.rs`, delete the `FlaggedView` type alias (lines 17-18), the `RootSection` struct (lines 20-31), the `render_view` function (lines 111-120), and the `render_section_from_raw` function (lines 122-133). Drop the `use serde::Serialize;` line if it has no other consumer (it does not after `RootSection` leaves). Insert the re-exports near the top, after the `pub use crate::state::DomainError;` line:

  ```rust
  pub use crate::web::render::{FlaggedView, RootSection};
  // Pre-fold helper names. The new homes are package_view and package_section in
  // web::render. These re-exports go away with service.rs itself.
  pub(crate) use crate::web::render::package_view as render_view;
  pub(crate) use crate::web::render::package_section as render_section_from_raw;
  ```

- [ ] **Step 4: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count. `service::tests` resolves `render_view`, `FlaggedView`, and `RootSection` through the re-exports. `web::render::tests` keeps working with the now-local types. `autosync` and `demo` still see the old names via the re-exports.

- [ ] **Step 5: Commit**

  ```bash
  git add src/web/render.rs src/service.rs
  git commit -m "refactor(render): move FlaggedView, RootSection, package_view, package_section to web/render.rs

  Relocate the four read-side packaging items from service.rs to
  web/render.rs, where they sit next to the markup that consumes them.
  Rename render_view to package_view and render_section_from_raw to
  package_section so they do not collide with the existing markup
  render_view in the same module.

  service.rs keeps re-exports under both old and new names so the
  remaining consumers (web.rs handlers, autosync, demo, the service test
  module) compile unchanged through the rest of the migration. The stubs
  go away with service.rs itself."
  ```

---

## Task 4: Move store-behavior tests from `service::tests` to `state::tests`

Thirteen tests under `service::tests` exercise the store and the marker IO. They call `current_view`, `rescan`, `mark`, and `unmark` indirectly through the wrapper layer that the later tasks delete. The destination (`state::tests`) already has the close cousins of several. Each test is audited: dropped as a duplicate, merged into an existing test, or ported with the wrapper call rewritten to a direct `store.X()` call.

This task lands before the handler folds because the wrappers must outlive the tests that call them.

**Files:**
- Modify: `src/state.rs` (ports and merges)
- Modify: `src/service.rs` (test block shrinks)

**Per-test action table** (line numbers reference today's `src/service.rs`):

| Test | Action |
| --- | --- |
| `cache_hit_within_ttl_serves_the_same_raw_slot` (284) | Drop. Duplicated by `state::tests::store_current_serves_stored_raw_within_ttl` (535). |
| `warm_concurrent_reads_share_one_raw_slot_and_render_equally` (303) | Port. Exercises warm-concurrent reads on a single slot, not the cold single-flight that `state::tests::store_current_single_flights_a_cold_slot` covers. Rewrite to call `store.current()` twice through `tokio::join!`. |
| `ttl_zero_rescans_every_call` (331) | Port. No counterpart in `state::tests`. Rewrite the two `current_view` calls to `store.current()`. |
| `rescan_refreshes_even_within_a_live_ttl` (355) | Port. No counterpart. Rewrite `current_view` -> `store.current()` and `rescan` -> `store.rescan()`. |
| `rescan_clears_the_dir_index_then_repopulates_it` (369) | Port. `state::tests::store_rescan_clears_the_dir_index` (562) is weaker (only checks the clear, not the repopulate). The port replaces it. Rewrite wrapper calls. |
| `a_warm_state_rescan_reuses_the_index` (428) | Port. No counterpart. Rewrite wrapper calls. |
| `mark_invalidates_the_marked_dir_in_the_index` (445) | Port. No counterpart. Rewrite `current_view` and `mark` calls. |
| `mark_updates_a_warm_cache_in_place_without_rescanning` (491) | Port. `state::tests::store_write_mark_edits_the_slot_in_place` (575) covers the slot in-place edit; the service test additionally asserts the slot is not rebuilt by a follow-up read. The port adds the second assertion to the state test (or lands as a new test if cleaner). |
| `mark_on_a_cold_cache_scans_fresh` (522) | Port. No counterpart. Rewrite the `mark` call to `store.write_mark()`. |
| `mark_outside_a_root_is_rejected` (541) | Port. `write_marker_rejects_an_escape` (640) covers the lower-level write_marker; the service version covers `store.write_mark` returning the typed error. Both stay. |
| `mark_with_a_bad_root_index_errors` (552) | Drop. Duplicated by `state::tests::store_write_mark_bad_root_index_errors` (603). |
| `unmark_deletes_the_file_and_re_flags_the_root` (562) | Drop. Duplicated by `state::tests::store_remove_mark_re_flags_the_root` (694). |
| `unmark_with_a_bad_root_index_errors` (590) | Port. No counterpart (state covers write_mark but not remove_mark for the bad-index case). Rewrite `unmark` -> `store.remove_mark()`. |

Plus drop the `state_for(root, ttl_seconds)` test helper (line 277); `state::tests::test_store` covers the same setup pattern directly against `RawViewStore`. Drop `test_config`, `test_settings`, `test_index` after their last call site goes.

**Migration recipe** (apply to each ported test):

Before (in `service::tests`, calling the wrapper):

```rust
let state = state_for(dir.path(), 600);
let _first = current_view(&state, ViewMode::GapsOnly).await;
let after = mark(&state, 0, "Book", Marker::NoEbook, ViewMode::GapsOnly)
    .await
    .unwrap();
assert!(matches!(after.view[0].state, RootState::Clean));
```

After (in `state::tests`, calling the store directly):

```rust
let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
let _first = store.current().await;
let applied = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
assert!(applied.created);
// The render-shape assertion moves to render::tests in Task 5. In
// state::tests, assert on the raw view by pattern-matching the RootScan.
let RootScan::Walked { folders, .. } = &applied.raw[0] else {
    panic!("walked root");
};
let book = folders.iter().find(|f| f.rel_path.as_os_str() == "Book").unwrap();
assert!(!book.missing_ebook, "the marker write cleared the gap on the raw view");
```

Note the assertion shift. The service test asserted on the `FlaggedView`'s `RootState::Clean`. In `state::tests`, the right assertion is on the raw view (`folders[i].missing_ebook` reached through the `RootScan::Walked` arm) since that is the layer being tested. The packaging-into-`FlaggedView` is its own test in Task 5. Apply the same shift everywhere a ported test asserted on `RootState` or `Forest` shapes. The existing `store_write_mark_edits_the_slot_in_place` test (line 575) shows the simpler pattern when only `applied.created` and the slot identity matter; reuse that shape when the ported assertion does not actually need the folder-level detail.

For the warm-concurrent test, the join shape stays:

```rust
let store = Arc::new(test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf()));
let _warm = store.current().await;
let before = store.peek_stored_arc().await.expect("warmed slot");
let s1 = Arc::clone(&store);
let s2 = Arc::clone(&store);
let (a, b) = tokio::join!(s1.current(), s2.current());
assert!(Arc::ptr_eq(&a, &b), "warm concurrent reads share one Arc");
let after = store.peek_stored_arc().await.expect("warmed slot");
assert!(Arc::ptr_eq(&before, &after), "warm concurrent reads did not rebuild");
```

- [ ] **Step 1: Apply the per-test action table to `state::tests`**

  For each test marked "Port" in the table, copy the test body from `service::tests` to `state::tests`, rewrite the wrapper calls to direct `store.X()` calls, and shift any `FlaggedView` / `RootState` assertions to `RawView` / `ScannedFolder` assertions. Place each ported test in `state::tests` in the order shown in the table, grouped after the existing `store_*` tests. Use the existing `test_store` helper.

  For each test marked "Drop", do nothing in `state::tests`; it stays where it is in `service::tests` until Step 2.

  For each test marked "Merge", extend the named existing `state::tests` test in place (do not create a new test).

- [ ] **Step 2: Delete the migrated and duplicated tests from `service::tests`**

  In `src/service.rs`, delete the 13 store-behavior tests by name (use the table). Also delete the `state_for` helper (line 277). The `test_config`, `test_settings`, and `test_index` helpers stay for now since the render-packaging tests in Task 5 still need them; delete them in Task 5.

- [ ] **Step 3: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count as before this task, minus the three deliberate duplicate drops. The ported tests assert on the raw view, the dropped tests do not run, the surviving service tests still compile and pass.

- [ ] **Step 4: Commit**

  ```bash
  git add src/state.rs src/service.rs
  git commit -m "chore(tests): move store-behavior tests from service to state::tests

  Port ten store-behavior tests from service::tests to state::tests,
  rewriting their wrapper calls (current_view, rescan, mark, unmark) to
  direct store.X() calls and shifting their FlaggedView assertions onto
  the raw view. Drop three tests that duplicated existing state::tests
  cases (cache_hit_within_ttl_serves_the_same_raw_slot,
  mark_with_a_bad_root_index_errors, and
  unmark_deletes_the_file_and_re_flags_the_root). Drop the state_for
  helper since state::tests::test_store covers the same setup.

  service::tests still owns the render-packaging tests, the type-shape
  test, the ViewMode tests, and the audiobook_count test; those move in
  Tasks 5 and 6."
  ```

---

## Task 5: Move render-packaging and type-shape tests to `render::tests`

Six tests under `service::tests` exercise the packaging step (`render_view` / `render_section_from_raw`, soon `package_view` / `package_section`) and one exercises `RootSection`'s `Serialize` impl. They move to `web/render.rs::tests` where the types and helpers now live.

**Files:**
- Modify: `src/web/render.rs` (gains six packaging tests and one serialize test)
- Modify: `src/service.rs` (test block shrinks further)

**Tests to move** (with the new call names):

| Test | After |
| --- | --- |
| `root_with_a_gap_yields_a_matching_forest` (163) | Call `package_view(&raw, ViewMode::GapsOnly)` instead of `render_view(&raw, ViewMode::GapsOnly)`. |
| `root_with_no_audio_is_clean` (180) | Same rename. |
| `missing_root_is_error_and_other_roots_still_render` (190) | Same rename. |
| `render_view_computes_total_audiobooks_per_root` (251) | Same rename. Rename the test itself to `package_view_computes_total_audiobooks_per_root` for accuracy. |
| `all_mode_builds_the_full_tree_including_covered_folders` (622) | Same rename. |
| `root_states_serialize_to_stable_json` (470) | Pure type-shape test on `RootState` and `RootSection`. Moves verbatim. |

The five packaging tests need `build_view` from `state` (today they import `crate::state::build_view`). That import stays. They need `test_config`, `test_settings`, `test_index`; copy these into the receiving module if `render::tests` does not have analogs. Check `render::tests` first; the existing fixture helpers (`section`, `clean`, `errored`, etc. from Task 1 of the prior render-page-tests migration) cover synthetic shapes but not real-scan setup. Copy the three helpers.

- [ ] **Step 1: Read the existing `render::tests` block**

  Open `src/web/render.rs` at the bottom (around line 689 in today's file). The block has `extract_attr_value`, the OOB-pin test, plus the fixture helpers `flagged_leaf`, `covered_leaf`, `container`, `forest`, `section`, `clean`, `errored` added by the render-page-tests migration. There is no `test_config` / `test_settings` / `test_index` analog; those will be copied in Step 3.

- [ ] **Step 2: Move the six tests to `render::tests`**

  Copy each test body from `src/service.rs` (use the line numbers above) into `src/web/render.rs::tests`. Apply the function-name rewrites (`render_view` -> `package_view`, `render_section_from_raw` -> `package_section`). For `render_view_computes_total_audiobooks_per_root`, rename to `package_view_computes_total_audiobooks_per_root`.

  Place the packaging tests after the existing synthetic-fixture tests; place `root_states_serialize_to_stable_json` next to where `RootSection` is described or near the other type-shape tests if the block has them.

- [ ] **Step 3: Copy the integration test helpers**

  Add to `render::tests` (the helpers `test_config`, `test_settings`, `test_index` from `service::tests` lines 146-160):

  ```rust
  use crate::config::Config;
  use crate::scanner::ScanSettings;
  use crate::state::build_view;
  use std::path::PathBuf;

  fn test_config(roots: Vec<PathBuf>, ttl_seconds: u64) -> Config {
      Config {
          library_roots: roots,
          ttl_seconds,
          ..Default::default()
      }
  }

  fn test_settings() -> Arc<ScanSettings> {
      Arc::new(ScanSettings::compile(Config::default().scan_inputs()).unwrap())
  }

  fn test_index() -> Arc<std::sync::Mutex<crate::scanner::DirIndex>> {
      Arc::new(std::sync::Mutex::new(crate::scanner::DirIndex::new()))
  }
  ```

  Add `use std::sync::Arc;` if not already in `render::tests`'s scope.

- [ ] **Step 4: Delete the moved tests from `service::tests`**

  In `src/service.rs`, delete the six tests by name. Also delete the `test_config`, `test_settings`, `test_index` helpers (lines 146-160) since `service::tests` no longer holds any test that uses them.

- [ ] **Step 5: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count as before this task. The six tests now live in `web::render::tests`; the type-shape test asserts on `RootSection` in its new home; the four `ViewMode` tests and the `audiobook_count` test still sit in `service::tests` awaiting Task 6.

- [ ] **Step 6: Commit**

  ```bash
  git add src/web/render.rs src/service.rs
  git commit -m "chore(tests): move render-packaging and type-shape tests from service to render::tests

  Port six tests from service::tests to web::render::tests:
  - five packaging tests (root_with_a_gap_yields_a_matching_forest,
    root_with_no_audio_is_clean,
    missing_root_is_error_and_other_roots_still_render,
    package_view_computes_total_audiobooks_per_root,
    all_mode_builds_the_full_tree_including_covered_folders) call
    package_view / package_section directly at their new home.
  - root_states_serialize_to_stable_json asserts on RootSection's
    Serialize impl next to the struct.

  Carry the test_config / test_settings / test_index helpers from
  service::tests; render::tests' synthetic-fixture helpers do not cover
  real-scan setup."
  ```

---

## Task 6: Move `ViewMode` tests to `tree::tests` and the `audiobook_count` test to `scanner::tests`

Four tests remain under `service::tests` that have nothing to do with `service` at all. Three exercise `ViewMode` (which lives in `tree.rs`); one exercises `RootScan::audiobook_count` (which lives in `scanner.rs`).

**Files:**
- Modify: `src/tree.rs` (gains three `ViewMode` tests)
- Modify: `src/scanner.rs` (gains one `audiobook_count` test)
- Modify: `src/service.rs` (test block becomes empty)

**Tests to move:**

| Test | Destination |
| --- | --- |
| `view_mode_parses_the_query_token_leniently` (599) | `tree::tests` |
| `view_mode_round_trips_through_its_query_token` (608) | `tree::tests` |
| `view_mode_path_returns_canonical_url_per_mode` (615) | `tree::tests` |
| `audiobook_count_counts_walked_folders_that_directly_hold_audio` (206) | `scanner::tests` |

The three `ViewMode` tests are pure (no IO, no fixtures); they move verbatim.

The `audiobook_count` test constructs `RootScan::Walked` and `RootScan::Failed` literals and calls `walked.audiobook_count()`. It also moves verbatim; the imports it needs (`scanner::{RootScan, ScannedFolder}`, `std::path::PathBuf`) resolve locally inside `scanner::tests`.

- [ ] **Step 1: Read the receiving test blocks**

  Open `src/tree.rs` and confirm a `mod tests` block exists. Same for `src/scanner.rs`. If either does not have one, add `#[cfg(test)] mod tests { use super::*; }` and continue.

- [ ] **Step 2: Move the three `ViewMode` tests**

  Copy the three test bodies from `src/service.rs:599-619` to `src/tree.rs::tests`. They are pure functions; no fixture helpers needed.

- [ ] **Step 3: Move the `audiobook_count` test**

  Copy the test body from `src/service.rs:206-248` to `src/scanner.rs::tests`. Remove the `use crate::scanner::{RootScan, ScannedFolder};` line (it would be redundant after the move; `super::*` covers it). The `use std::path::PathBuf;` can be either at the top of the test or kept inline.

- [ ] **Step 4: Delete the four moved tests from `service::tests`**

  In `src/service.rs`, delete the four tests by name. After this step, `mod tests` in `service.rs` should be empty (just `use super::*;` and a few stray imports). Delete those too. Leave the empty `#[cfg(test)] mod tests {}` block; Task 13 deletes the whole file.

- [ ] **Step 5: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count. The four tests pass in their new homes.

- [ ] **Step 6: Commit**

  ```bash
  git add src/tree.rs src/scanner.rs src/service.rs
  git commit -m "chore(tests): move ViewMode tests to tree::tests and audiobook_count test to scanner::tests

  Port three ViewMode tests (parses_the_query_token_leniently,
  round_trips_through_its_query_token, path_returns_canonical_url_per_mode)
  from service::tests to tree::tests, where ViewMode lives. Port
  audiobook_count_counts_walked_folders_that_directly_hold_audio to
  scanner::tests next to RootScan.

  service::tests is now empty; the production wrappers go in Tasks 7-10
  and the file itself in Task 13."
  ```

---

## Task 7: Inline `service::current_view` into `web::index` and the main warm-up

The wrapper does: `store.current().await` then `package_view`, wrap in `Arc::new`. The handler discards the Arc immediately (renders directly off `&view`); the warm-up discards the return value entirely. Inline both, drop the Arc, delete the wrapper.

**Files:**
- Modify: `src/web.rs` (handler `index` absorbs the wrapper)
- Modify: `src/main.rs` (warm-up loses the wrapper call)
- Modify: `src/service.rs` (delete `current_view`)

**Interfaces produced:** none new; the wrapper is removed.

- [ ] **Step 1: Rewrite `web::index`**

  Open `src/web.rs:65-79`. Replace the body with:

  ```rust
  async fn index(State(state): State<Arc<AppState>>, Query(query): Query<ViewQuery>) -> Html<String> {
      let started = Instant::now();
      let mode = ViewMode::from_query(query.view.as_deref());
      let raw = state.store.current().await;
      let view = render::package_view(&raw, mode);
      let render_started = Instant::now();
      let html = render::render_view(&view, &state.config.search_links, mode).into_string();
      tracing::debug!(
          op = "index",
          mode = mode.as_query(),
          render_ms = render_started.elapsed().as_secs_f64() * 1e3,
          elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
          "handled request"
      );
      Html(html)
  }
  ```

  The tracing fields and the `render_started` clock stay; the handler is otherwise mechanical.

- [ ] **Step 2: Rewrite the warm-up in `main.rs`**

  Open `src/main.rs:88-98`. Replace the spawned block's body with:

  ```rust
  tokio::spawn({
      let state = Arc::clone(&state);
      async move {
          // Warm the gaps-mode slot. The packaging is cheap; the cache
          // slot side effect is what we want.
          let _ = state.store.current().await;
          tracing::debug!("startup cache warm complete");
      }
  });
  ```

  Drop the `missing_ebooks::service::current_view` and `missing_ebooks::tree::ViewMode::GapsOnly` path expressions.

- [ ] **Step 3: Delete `service::current_view`**

  In `src/service.rs`, delete the function (lines 54-59). Drop the `Arc` import line if it has no other consumer (it does: `current_view` was its only user, but `mark` / `unmark` still use it; keep it for now).

- [ ] **Step 4: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count. The handler-level tests in `web::tests` still run against the router; behavior is unchanged.

- [ ] **Step 5: Commit**

  ```bash
  git add src/web.rs src/main.rs src/service.rs
  git commit -m "refactor(web): inline service::current_view into web::index and main warm-up

  web::index now reads store.current() directly, packages the raw view
  via render::package_view, and renders. The Arc<FlaggedView> wrapper
  the deleted wrapper added on top of the store's Arc<RawView> is gone;
  the handler holds FlaggedView by value for the duration of one
  response.

  The main.rs warm-up drops the service::current_view call entirely; a
  bare store.current().await provides the same cache-slot side effect.
  service::current_view itself is deleted in this commit."
  ```

---

## Task 8: Inline `service::rescan` into `web::rescan`

Same shape as Task 7. The wrapper is three lines; the handler absorbs them.

**Files:**
- Modify: `src/web.rs` (handler `rescan` absorbs the wrapper)
- Modify: `src/service.rs` (delete `rescan`)

- [ ] **Step 1: Rewrite `web::rescan`**

  Open `src/web.rs:171-186`. Replace the body with:

  ```rust
  async fn rescan(State(state): State<Arc<AppState>>, Form(query): Form<ViewQuery>) -> Response {
      let started = Instant::now();
      let mode = ViewMode::from_query(query.view.as_deref());
      let raw = state.store.rescan().await;
      let view = render::package_view(&raw, mode);
      // Swap the fresh sections into #roots and push the mode path, so the
      // address bar tracks the view without ever showing the /rescan POST URL.
      let markup = render::roots(&view, &state.config.search_links, mode);
      let resp = ([("HX-Push-Url", mode.path())], Html(markup.into_string())).into_response();
      tracing::debug!(
          op = "rescan",
          mode = mode.as_query(),
          elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
          "handled request"
      );
      resp
  }
  ```

- [ ] **Step 2: Delete `service::rescan`**

  In `src/service.rs`, delete the function (lines 61-66).

- [ ] **Step 3: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count.

- [ ] **Step 4: Commit**

  ```bash
  git add src/web.rs src/service.rs
  git commit -m "refactor(web): inline service::rescan into web::rescan

  web::rescan now reads store.rescan() directly, packages the result
  with render::package_view, builds the #roots swap markup, and attaches
  HX-Push-Url. service::rescan is deleted."
  ```

---

## Task 9: Inline `service::mark` into `web::mark`; drop `MarkOutcome`

The handler reads `Applied { raw, created }` straight off the store. `MarkOutcome` and the `Arc::new(render_view(...))` wrapping inside it both disappear. `failed_write_response` keeps its current shape but loses the `service::current_view` call inside it (replaced by `store.current()` + `package_view`).

**Files:**
- Modify: `src/web.rs` (handler `mark` and `failed_write_response` absorb the wrappers)
- Modify: `src/service.rs` (delete `mark`, delete `MarkOutcome`)

- [ ] **Step 1: Rewrite `web::mark`**

  Open `src/web.rs:81-117`. Replace the body with:

  ```rust
  async fn mark(
      State(state): State<Arc<AppState>>,
      Form(req): Form<MarkRequest>,
  ) -> axum::response::Response {
      let started = Instant::now();
      let links = &state.config.search_links;
      let mode = req.view;
      let resp = match state.store.write_mark(req.root, &req.rel, req.kind).await {
          Ok(applied) => {
              let view = render::package_view(&applied.raw, mode);
              let markup =
                  render::render_section(&view[req.root], req.root, None, links, mode);
              let trigger = applied.created.then(|| {
                  let name = display_name(&view[req.root].path, &req.rel);
                  marked_trigger(&req, &name)
              });
              section_response(markup, trigger)
          }
          Err(err) => {
              failed_write_response(
                  &state,
                  req.root,
                  mode,
                  links,
                  format!("Could not mark {}: {err}", req.rel),
              )
              .await
          }
      };
      tracing::debug!(
          op = "mark",
          root = req.root,
          rel = %req.rel,
          elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
          "handled request"
      );
      resp
  }
  ```

- [ ] **Step 2: Rewrite `failed_write_response`**

  Open `src/web.rs:152-169`. Replace the body with:

  ```rust
  /// Re-render the affected root's section with an inline alert naming the
  /// folder, so a failed write stays by the row rather than in a toast. The view
  /// is re-fetched (a cache hit) since the failed call returned no view. An
  /// out-of-range root falls back to a standalone error card.
  async fn failed_write_response(
      state: &AppState,
      root: usize,
      mode: ViewMode,
      links: &[SearchLink],
      message: String,
  ) -> axum::response::Response {
      let raw = state.store.current().await;
      let view = render::package_view(&raw, mode);
      let markup = match view.get(root) {
          Some(section) => render::render_section(section, root, Some(&message), links, mode),
          None => render::error_section(root, &message),
      };
      section_response(markup, None)
  }
  ```

- [ ] **Step 3: Delete `service::mark` and `MarkOutcome`**

  In `src/service.rs`, delete the `MarkOutcome` struct (lines 69-78) and the `mark` function (lines 80-95). Keep the `use crate::marker::Marker;` line; `unmark` still uses it (Task 10 drops it).

- [ ] **Step 4: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count. The `web::tests` block in `web.rs` (the handler-shape tests that survived the render-page-tests migration) boots the router and exercises the new handler body; assertions are unchanged.

- [ ] **Step 5: Commit**

  ```bash
  git add src/web.rs src/service.rs
  git commit -m "refactor(web): inline service::mark into web::mark; drop MarkOutcome

  web::mark now calls store.write_mark directly and reads Applied.created
  off the result inline. The MarkOutcome packaging type dissolves; its
  two fields were already what the handler wanted, just one indirection
  deeper. failed_write_response loses its service::current_view call too,
  calling store.current() + render::package_view directly.

  The Arc<FlaggedView> wrapper around the mark response is gone; the
  handler builds a FlaggedView by value, borrows one &RootSection for
  the section response, and drops the view at the end of the block.

  service::mark and service::MarkOutcome are deleted."
  ```

---

## Task 10: Inline `service::unmark` into `web::unmark`

Mirror of Task 9, simpler: no `MarkOutcome`, no `created` field.

**Files:**
- Modify: `src/web.rs` (handler `unmark` absorbs the wrapper)
- Modify: `src/service.rs` (delete `unmark`)

- [ ] **Step 1: Rewrite `web::unmark`**

  Open `src/web.rs:119-150`. Replace the body with:

  ```rust
  async fn unmark(
      State(state): State<Arc<AppState>>,
      Form(req): Form<MarkRequest>,
  ) -> axum::response::Response {
      let started = Instant::now();
      let links = &state.config.search_links;
      let mode = req.view;
      let resp = match state.store.remove_mark(req.root, &req.rel, req.kind).await {
          Ok(raw) => {
              let view = render::package_view(&raw, mode);
              section_response(
                  render::render_section(&view[req.root], req.root, None, links, mode),
                  None,
              )
          }
          Err(err) => {
              failed_write_response(
                  &state,
                  req.root,
                  mode,
                  links,
                  format!("Could not undo {}: {err}", req.rel),
              )
              .await
          }
      };
      tracing::debug!(
          op = "unmark",
          root = req.root,
          rel = %req.rel,
          elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
          "handled request"
      );
      resp
  }
  ```

  Note: `store.remove_mark` returns `Result<Arc<RawView>, DomainError>`. The local `raw` is `Arc<RawView>`; `&raw` dereferences to `&RawView` for `package_view`.

- [ ] **Step 2: Delete `service::unmark` and clean up unused imports**

  In `src/service.rs`, delete the function (lines 97-109). At this point the file holds only the four re-exports and an empty `#[cfg(test)] mod tests` block. Drop every remaining `use` line at the top of the file (`std::sync::Arc`, `crate::marker::Marker`, `crate::state::{self, AppState}`, `crate::scanner`, `crate::tree::{...}`, and any other surviving import). The re-exports use fully-qualified paths and need nothing imported.

- [ ] **Step 3: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count.

- [ ] **Step 4: Commit**

  ```bash
  git add src/web.rs src/service.rs
  git commit -m "refactor(web): inline service::unmark into web::unmark

  web::unmark now calls store.remove_mark directly, packages the
  refreshed raw view via render::package_view, and renders the affected
  section. The Arc<FlaggedView> wrapper the deleted wrapper added on top
  of the store's Arc<RawView> is gone.

  service::unmark is deleted. After this commit, service.rs holds only
  the FlaggedView/RootSection re-exports, the DomainError re-export, the
  package_view/package_section re-exports under their pre-fold names,
  and an empty test module."
  ```

---

## Task 11: Rewire `autosync` to call `web::render::package_section`

`autosync.rs` calls `crate::service::render_section_from_raw` in one place and references it in two doc comments. After this rewire, `service.rs` has no `autosync` consumer for the pre-fold name.

**Files:**
- Modify: `src/autosync.rs`

- [ ] **Step 1: Rewrite the call site and doc comments**

  Open `src/autosync.rs`. Find the line `let rendered_section = crate::service::render_section_from_raw(raw_section, mode);` (around line 75) and replace with:

  ```rust
  let rendered_section = crate::web::render::package_section(raw_section, mode);
  ```

  Find the second call site (around line 696) and apply the same rewrite.

  Update the doc comments at lines 64-65 and 685-686 to reference `web::render::package_section` instead of `service::render_section_from_raw`. The prose stays: "one place owns the raw to packaged step" reads the same way.

- [ ] **Step 2: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count.

- [ ] **Step 3: Commit**

  ```bash
  git add src/autosync.rs
  git commit -m "refactor(autosync): rewire render_section_from_raw to web::render::package_section

  Update the one call site and two doc-comment mentions to use the new
  home and name. service.rs's pre-fold re-export goes away with the file
  in the final commit."
  ```

---

## Task 12: Rewire `demo` to call `web::render::package_view`

`demo/handlers.rs` imports `FlaggedView` and `render_view` from `service`. After this rewire, the demo bypasses the re-export and points at `web::render` directly.

**Files:**
- Modify: `src/demo/handlers.rs`

- [ ] **Step 1: Rewrite the import and call site**

  Open `src/demo/handlers.rs`. Replace:

  ```rust
  use crate::service::{FlaggedView, render_view};
  ```

  with:

  ```rust
  use crate::web::render::{FlaggedView, package_view};
  ```

  Find the call inside `derive_view` (around line 147):

  ```rust
  render_view(&raw, mode)
  ```

  Replace with:

  ```rust
  package_view(&raw, mode)
  ```

- [ ] **Step 2: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count. The demo's session-aware view derivation keeps working; only the function path changed.

- [ ] **Step 3: Commit**

  ```bash
  git add src/demo/handlers.rs
  git commit -m "refactor(demo): rewire render_view to web::render::package_view

  demo::handlers::derive_view now imports FlaggedView and package_view
  from web::render directly. service.rs's pre-fold re-export goes away
  with the file in the final commit."
  ```

---

## Task 13: Delete `src/service.rs`

After Tasks 11 and 12, `service.rs` holds only re-exports and an empty test module. No consumer references `service::*` anywhere in the crate. The file and the module declaration in `lib.rs` can go.

**Files:**
- Delete: `src/service.rs`
- Modify: `src/lib.rs` (drop `pub mod service;`)

- [ ] **Step 1: Confirm no remaining `service` references**

  ```bash
  rg -n "service::|use crate::service|crate::service::" --type rust src/
  ```

  Expected: no output. (The `service` literal might still appear in comments or strings; spot-check that any matches are not active code paths.)

- [ ] **Step 2: Delete the file and drop the module declaration**

  ```bash
  rm src/service.rs
  ```

  Open `src/lib.rs`. Delete the `pub mod service;` line.

- [ ] **Step 3: Run the suite**

  ```bash
  cargo test
  ```

  Expected: same passing count. Build is green; no `service` symbol exists anywhere.

- [ ] **Step 4: Run the pre-commit hook locally**

  ```bash
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo doc -D warnings
  ```

  Expected: all three pass. If `cargo doc` reports an intra-doc-link warning naming `crate::service`, find the doc comment and rewrite it to point at the new home.

- [ ] **Step 5: Commit**

  ```bash
  git add src/lib.rs src/service.rs
  git commit -m "chore: delete src/service.rs

  Remove the file and its module declaration from lib.rs. All of its
  symbols moved to their new homes in Tasks 2-12:
  - FlaggedView, RootSection, package_view, package_section in web/render.rs
  - DomainError in state.rs
  - the four async wrappers inlined into web handlers (current_view into
    index, rescan into rescan, mark into mark, unmark into unmark)
  - MarkOutcome dissolved into Applied { raw, created } reads at the
    handler
  - Arc<FlaggedView> wrappers dropped

  No remaining service:: path exists in the codebase. See ADR-0028."
  ```

---

## Verification

After Task 13:

- `cargo test` passes with the same count as before Task 1 (the test moves are renames and de-duplications, not deletions of unique coverage; the three drops are duplicates already covered in `state::tests`).
- `cargo clippy --all-targets -- -D warnings` is clean.
- `cargo doc -D warnings` is clean.
- `git log --oneline` shows the 13 commits in order, all with conventional-commit subjects.
- `find src -name service.rs` returns nothing.
- `rg "service::" src/` returns nothing.

Optional UI verification (recommended after Task 10 and again after Task 13): run the seeded harness and click through to confirm the rendered shapes are identical.

```bash
cargo run --example explore -- mixed-forest --port 8919
```

(Check `lsof -iTCP:8919 -sTCP:LISTEN` first; pick a different port if it is taken, per `CLAUDE.md`.)
