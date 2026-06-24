# tree.rs owns the ADR-0005 `.`-node rule: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Concentrate ADR-0005's `.`-node rule and the canonical-path name extraction into `tree::build`, and move `RootState`/`ViewMode` next to `Node` in `tree.rs`.

**Architecture:** Pure refactor, no behavior change. The renderer reaches the same `RootState` variants through the same forest-construction path; only the seam shape changes. Three logical commits: (A) move types, (B) reshape `tree::build` and delete `render_root_state`, (C) tighten doc comments and ADR-0005 cross-references.

**Tech Stack:** Rust 2024, `cargo test`, `cargo clippy -- -D warnings`, `cargo doc -D warnings`. The committed `.githooks/pre-commit` runs fmt + clippy + doc + the accent test on asset changes; do not bypass with `--no-verify`.

## Global Constraints

- Linear history on `main`. Granular commits, never squashed.
- Conventional Commits (`type(scope): subject`). A pure refactor with no behavior change uses `refactor`, with a body explaining the move and noting `No behavior change.`.
- Prose-only changes use the `Prose only: ...` caveat in the body.
- No `--no-verify`. The pre-commit hook is the bar.
- The spec lives at `.scratch/tree-owns-dot-node/README.md`; update it as the implementation progresses.

---

## File structure

- `src/tree.rs` (modify): gains `RootState`, `ViewMode`, and the new `build(scan, mode) -> RootState` signature. Keeps `Node` and the private forest-construction path.
- `src/service.rs` (modify): loses `RootState` (~10 lines), `ViewMode` (~40 lines including `impl`), and `render_root_state` (~30 lines). Imports the moved types from `tree`. `render_section_from_raw` becomes a four-line struct literal.
- `src/web.rs`, `src/web/page.rs`, `src/web/render.rs`, `src/autosync.rs`, `src/state.rs`, `src/main.rs`, `src/demo/banner.rs`, `src/demo/handlers.rs`, `tests/cache_render_byte_equal.rs` (modify): each updates its `use crate::service::{...}` imports to pull `ViewMode` and `RootState` from `crate::tree` instead.

A re-export pattern (`pub use crate::tree::{RootState, ViewMode}` from `service`) is rejected: it leaves the conceptual stack ambiguous and the goal is concentration, not aliasing.

---

### Task 1: Move `RootState` and `ViewMode` from `service.rs` to `tree.rs`

**Files:**
- Modify: `src/tree.rs` (add the two type definitions near the top, after the module doc-comment and the `Node` struct)
- Modify: `src/service.rs` (remove the two definitions, add `pub use crate::tree::{RootState, ViewMode}` for one commit's worth of compatibility? **No.** Update imports directly.)
- Modify: `src/web.rs`, `src/web/page.rs`, `src/web/render.rs`, `src/autosync.rs`, `src/state.rs`, `src/main.rs`, `src/demo/banner.rs`, `src/demo/handlers.rs`, `tests/cache_render_byte_equal.rs` (every `use crate::service::{..., ViewMode, ..., RootState, ...}` rewires)

**Interfaces:**
- Produces: `crate::tree::ViewMode` (identical shape to today's `crate::service::ViewMode`, including `Default`, `Deserialize`, `enum_map::Enum`, `from_query`, `as_query`, `path`). `crate::tree::RootState` (identical shape, including `Serialize` with `rename_all = "snake_case"`).
- Consumes: `crate::scanner::ScannedFolder` and `crate::scanner::reduce_to_flagged` already used by `tree.rs`; no new scanner dependency in this task.

- [ ] **Step 1: Verify the baseline is green**

Run: `cargo test --all-targets`
Expected: PASS.

- [ ] **Step 2: Move `RootState` into `tree.rs`**

In `src/tree.rs`, after the `Node` struct, paste the `RootState` definition exactly as it exists in `service.rs:79-88`:

```rust
/// The result of scanning one root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootState {
    /// Flagged gaps were found. The forest is non-empty.
    Forest(Vec<Node>),
    /// The root resolved and scanned with no gaps.
    Clean,
    /// The root could not be scanned (missing, not a directory, or unreadable).
    Error(String),
}
```

Add `use serde::Serialize;` to the `tree.rs` imports if not already present.

Delete the same block from `src/service.rs`.

- [ ] **Step 3: Move `ViewMode` into `tree.rs`**

In `src/tree.rs`, near `RootState`, paste the `ViewMode` definition and its `impl` block exactly as they exist in `service.rs:20-60` (the enum, the `from_query` / `as_query` / `path` methods, and all derives). Add `use serde::Deserialize;` to `tree.rs` if not already present.

Delete the same blocks from `src/service.rs`.

- [ ] **Step 4: Rewire imports across the crate**

Run: `cargo build 2>&1 | rg "unresolved import|cannot find type" | head -40`

For each compile error pointing at `crate::service::ViewMode` or `crate::service::RootState`, change the import to `crate::tree::ViewMode` / `crate::tree::RootState`. Files expected to change:

- `src/web.rs`
- `src/web/page.rs`
- `src/web/render.rs`
- `src/autosync.rs`
- `src/state.rs`
- `src/main.rs`
- `src/demo/banner.rs`
- `src/demo/handlers.rs`
- `tests/cache_render_byte_equal.rs`

Internal `service.rs` self-references (`ViewMode::GapsOnly`, etc.) become `tree::ViewMode::GapsOnly` or get a fresh `use crate::tree::{RootState, ViewMode};` at the top.

- [ ] **Step 5: Run the suite**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings && cargo doc --no-deps -D warnings`
Expected: PASS on all three.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(tree): move RootState and ViewMode into tree.rs

The per-root view-shape model lives next to Node now. RootState was
already Forest(Vec<Node>) | Clean | Error(String), keyed on tree's own
type, and ViewMode is the input that picks which folders enter the
forest. service.rs imports both from tree from here on.

Pure relocation. No behavior change."
```

---

### Task 2: Reshape `tree::build` and delete `render_root_state`

**Files:**
- Modify: `src/tree.rs` (change `build`'s signature and body; update the six in-file tests)
- Modify: `src/service.rs` (delete `render_root_state`, inline the call at `render_section_from_raw`)

**Interfaces:**
- Consumes: `crate::scanner::RootScan` (the enum with `Walked { canonical_path, folders }` and `Failed { message, .. }`), `crate::scanner::reduce_to_flagged(&[ScannedFolder]) -> Vec<ScannedFolder>`. Both already used by `service.rs::render_root_state` today.
- Produces: `pub fn build(scan: &RootScan, mode: ViewMode) -> RootState`. Old signature `build(root_name: &str, folders: &[ScannedFolder]) -> Vec<Node>` is removed.

- [ ] **Step 1: Rewrite the failing tests first**

Each of the six in-file tests in `src/tree.rs::tests` that calls `build("Audiobooks", &folders)` (or similar) needs to migrate. Add the helper at the top of the `tests` module:

```rust
use std::path::PathBuf;
use crate::scanner::RootScan;

fn walked(name: &str, folders: Vec<crate::scanner::ScannedFolder>) -> RootScan {
    // The canonical_path's file_name supplies the display name for the .-node
    // when the loose-root case fires (ADR-0005). The /lib prefix is arbitrary
    // padding; only the last component matters.
    RootScan::Walked {
        canonical_path: PathBuf::from("/lib").join(name),
        folders,
    }
}
```

Then rewrite each `let forest = build(NAME, &folders);` call site as:

```rust
let scan = walked("Audiobooks", folders);
let state = build(&scan, ViewMode::All);
let RootState::Forest(forest) = state else { panic!("expected forest") };
```

For tests that today pre-filter via `reduce_to_flagged`, drop the pre-filter and pass `ViewMode::GapsOnly` instead, or keep the pre-filter and pass `ViewMode::All`; pick whichever reads cleaner per test. Both reach the same code path.

The six tests:
- `build_carries_audio_files_onto_a_flagged_leaf` (around L259)
- `build_all_carries_all_four_kinds_sorted` (around L315)
- `build_all_pins_a_loose_audio_root_as_the_dot_node` (around L345)
- `build_all_carries_cover_files_onto_the_node` (around L369)
- `build_all_carries_audio_files_onto_the_node` (around L388)
- `build_all_carries_root_cover_files_onto_the_dot_node` (around L398)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib tree::tests`
Expected: FAIL with "expected 2 arguments, found 1" or "no function or associated item named `build` found with signature ..." across all six tests. (The old signature still exists; the new call sites won't compile.)

- [ ] **Step 3: Rewrite `tree::build`**

In `src/tree.rs`, replace the existing `pub fn build(...) -> Vec<Node>` with:

```rust
/// Builds the `RootState` for one library root in the requested mode.
///
/// Dispatches over the `RootScan` variant, derives the display name from the
/// canonical path for the loose-root `.`-node (ADR-0005), filters with
/// `reduce_to_flagged` when `mode` is `ViewMode::GapsOnly`, and collapses an
/// empty forest to `RootState::Clean`.
#[must_use]
pub fn build(scan: &crate::scanner::RootScan, mode: ViewMode) -> RootState {
    use crate::scanner::RootScan;
    match scan {
        RootScan::Failed { message, .. } => RootState::Error(message.clone()),
        RootScan::Walked { canonical_path, folders } => {
            if folders.is_empty() {
                return RootState::Clean;
            }
            let root_name = canonical_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(".");
            let working: std::borrow::Cow<'_, [crate::scanner::ScannedFolder]> = match mode {
                ViewMode::GapsOnly => {
                    std::borrow::Cow::Owned(crate::scanner::reduce_to_flagged(folders))
                }
                ViewMode::All => std::borrow::Cow::Borrowed(folders.as_slice()),
            };
            let forest = build_forest(root_name, &working);
            if forest.is_empty() {
                RootState::Clean
            } else {
                RootState::Forest(forest)
            }
        }
    }
}
```

Rename today's `build` body to a private `fn build_forest(root_name: &str, folders: &[ScannedFolder]) -> Vec<Node>` containing the existing algorithm (the `root_entry` branch, `insert_all`, `sort_forest`, the `.`-node `insert(0, ...)`). The body does not change; only the function name and visibility do.

- [ ] **Step 4: Delete `render_root_state` and inline at the call site**

In `src/service.rs`, delete `render_root_state` (the `pub(crate) fn` and its doc comment, around L192-224).

Update `render_section_from_raw` (around L184) to call `tree::build` directly:

```rust
/// Builds one `RootSection` from a raw `RootScan` for the requested mode.
///
/// The single owner of the raw-to-rendered packaging. `render_view` calls it
/// on the snapshot path; `autosync::render_oob_section` calls it on the push
/// path. Any future per-root field lands here once.
pub(crate) fn render_section_from_raw(scan: &scanner::RootScan, mode: ViewMode) -> RootSection {
    RootSection {
        path: scan.display_path().to_string(),
        state: tree::build(scan, mode),
        total_audiobooks: scan.audiobook_count(),
    }
}
```

- [ ] **Step 5: Run the tree tests**

Run: `cargo test --lib tree::tests`
Expected: PASS on all six migrated tests.

- [ ] **Step 6: Run the full suite**

Run: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings && cargo doc --no-deps -D warnings`
Expected: PASS on all three. The integration test `tests/cache_render_byte_equal.rs` exercises the snapshot/SSE byte equality and is the highest-signal check that ADR-0024 is preserved.

- [ ] **Step 7: Visual verification**

Run the seeded UI harness with the loose-root scenario to confirm the `.`-node still renders pinned-first in both modes:

Check the port is free (other worktrees may be running it):

```bash
lsof -iTCP:8919 -sTCP:LISTEN
```

If free, run:

```bash
cargo run --example explore -- root-flagged --port 8919
```

Open <http://localhost:8919/> and verify the pinned `.`-node row appears at the top of its root. Toggle the view to `?view=all` and verify it still appears pinned. Stop the harness with Ctrl-C when done.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(tree): build takes &RootScan and owns the .-node rule

ADR-0005's two halves live together now. tree::build dispatches over
the RootScan variant, derives the loose-root display name from the
canonical path, applies reduce_to_flagged for GapsOnly, and returns
RootState directly. service.rs::render_root_state is gone; the one
call site in render_section_from_raw inlines tree::build.

No behavior change: the forest-construction algorithm is untouched,
only renamed to a private build_forest. Snapshot/SSE byte equality
(ADR-0024) is preserved because both paths still flow through one
tree::build."
```

---

### Task 3: Update the architecture-review README and the spec

**Files:**
- Modify: `.scratch/architecture-review-2026-06/README.md` (mark candidate #4 done, link the scratch dir)
- Modify: `.scratch/tree-owns-dot-node/README.md` (flip Status to "done", link the implementing commits)

**Interfaces:** None.

- [ ] **Step 1: Update the architecture-review status table**

In `.scratch/architecture-review-2026-06/README.md`, change the row for #4 from:

```
| 4 | `tree.rs` owns the ADR-0005 `.`-node rule end-to-end | Worth exploring | open |
```

to:

```
| 4 | `tree.rs` owns the ADR-0005 `.`-node rule end-to-end | Worth exploring | **done** (see `.scratch/tree-owns-dot-node/`) |
```

Update the "Suggested next pick" paragraph at the bottom so it no longer lists #4 as open.

- [ ] **Step 2: Update the spec status**

In `.scratch/tree-owns-dot-node/README.md`, change the Status section from "Spec drafted 2026-06-24. Not yet implemented." to "Implemented 2026-06-24." with the two implementing commit short SHAs appended.

- [ ] **Step 3: Commit**

```bash
git add .scratch/architecture-review-2026-06/README.md .scratch/tree-owns-dot-node/README.md
git commit -m "docs(arch-review): mark candidate #4 done

Prose only: tree.rs now owns the ADR-0005 .-node rule; update the
status table and the spec's status section."
```

---

## Self-review

- **Spec coverage:** Every section of the spec maps to a task. Module move -> Task 1. Signature reshape + `render_root_state` deletion -> Task 2. Test migration -> Task 2. ADR notes -> documented but no code change. Deletion test -> verified empirically by the diff in Task 2's commit.
- **Placeholder scan:** No TBDs or vague "handle edge cases". The `Cow` choice for the `working` slice is concrete; the fallback name `"."` is the same constant `service.rs:209` uses today.
- **Type consistency:** `tree::build(scan: &RootScan, mode: ViewMode) -> RootState` is referenced consistently across tasks. The private `build_forest(root_name: &str, folders: &[ScannedFolder]) -> Vec<Node>` is the renamed today's-body, not a new function.
- **Spec requirements covered:** name derivation (Task 2 Step 3), mode-aware filter (Task 2 Step 3), empty-folders collapse to `Clean` (Task 2 Step 3), `RootState`/`ViewMode` move (Task 1), test migration (Task 2 Step 1).
