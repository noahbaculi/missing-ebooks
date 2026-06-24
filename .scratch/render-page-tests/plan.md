# Render and page test relocation: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the 82 markup-shape `#[tokio::test]` cases out of `src/web.rs::tests` and into `src/web/render.rs::tests` and `src/web/page.rs::tests`, calling the renderer and page-shell helpers directly on hand-built `FlaggedView` fixtures. The `web.rs` test surface shrinks to handler-shape only.

**Architecture:** No production code changes. The migration is mechanical: replace each test's `app_for(dir.path()).oneshot(...) -> body_string(response).await` with a direct call (`render_view(&view, &links, mode).into_string()`, `render_section(&section, ...).into_string()`, `page::page(mode, body).into_string()`, or a helper like `settings_menu().into_string()`). The assertion lines themselves stay byte-for-byte where today's substring is a renderer output. Each cluster is one commit.

**Tech Stack:** Rust 2024 edition, `maud` for HTML, `axum` and `tower` (used only by the handler-shape tests that stay). Test framework: `cargo test`. Fixture style is hand-built `Vec<Node>` / `RootSection` / `FlaggedView` literals, modeled on `tests/curated_contract.rs`.

## Global constraints

- Code-comments style (see `writing-style-code-comments`): terse, verb-first or noun-phrase doc summaries, no em dashes, no "This function" openers, backticks around identifiers and literals.
- No em dashes in any prose, ever (`AGENTS.md`).
- Commits follow Conventional Commits (`type(scope): subject`). Use `refactor(tests)` for the migrations, `chore(tests)` for the cleanup, `test(render)` and `test(page)` for additive helper commits if any. Granular, no squashing.
- Pre-commit hook runs `cargo fmt`, `cargo clippy`, `cargo doc -D warnings`. Never bypass with `--no-verify`.
- After each task: `cargo test` must pass.
- Spec: `.scratch/render-page-tests/spec.md`. Read it before starting Task 1 if any cluster boundary is unclear.

## Global migration pattern

Every cluster task follows the same shape. The diff is local: delete one test in `web.rs::tests`, add one test in `render::tests` or `page::tests`.

**Before** (in `src/web.rs::tests`):

```rust
#[tokio::test]
async fn some_markup_test() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("Book/01.mp3"));
    let body = body_string(
        app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(body.contains("..."));
}
```

**After** (in `src/web/render.rs::tests`):

```rust
#[test]
fn some_markup_test() {
    let view = vec![section(
        "/lib",
        RootState::Forest(vec![
            flagged_leaf("Book", "Book", &["01.mp3"]),
        ]),
        1,
    )];
    let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
    assert!(html.contains("..."));
}
```

Notes that apply to every migration:

- The test becomes synchronous (`#[test]`, not `#[tokio::test]`).
- Drop `app_for(...).oneshot(...) -> body_string(response)`. Call the renderer directly.
- `&links` is `&[]` unless the test specifically pins search-link shape.
- `total_audiobooks` matches the count the scanner would have produced from the synthetic tempdir: one folder that directly holds audio counts as one audiobook.
- A test that today writes to `dir.path().join("X/Y.mp3")` exposes the shape: one root, one nested forest. The fixture mirrors that shape exactly.
- Assertion bodies stay byte-for-byte where the substring is renderer output. If a test asserts on body bytes that come from `page::page` (head, navbar, dialog), it moves to `page::tests` instead.

## File map

- Modify: `src/web/render.rs`: add fixture helpers and 8 clusters of tests under `mod tests`.
- Modify: `src/web/page.rs`: add `#[cfg(test)] mod tests` block and 4 clusters of tests.
- Modify: `src/web.rs`: delete the migrated tests; trim helpers in the cleanup task.
- Out of scope: `tests/curated_contract.rs`, `tests/cache_render_byte_equal.rs`, `tests/sse.rs`, `tests/accent/`, every other `mod tests` block.

---

## Task 1: Add fixture helpers under `render::tests`

Introduces the synthetic-fixture helpers every later render-cluster task uses. No new test assertions; the existing OOB-pin test stays untouched and still passes.

**Files:**
- Modify: `src/web/render.rs` (extend `mod tests`)

**Interfaces produced:**

- `fn flagged_leaf(name: &str, rel: &str, audio: &[&str]) -> Node`
- `fn covered_leaf(name: &str, rel: &str, cover_files: &[&str]) -> Node`
- `fn container(name: &str, rel: &str, children: Vec<Node>) -> Node`
- `fn forest(roots: Vec<Node>) -> RootState`
- `fn section(path: &str, state: RootState, total: usize) -> RootSection`
- `fn clean(path: &str, total: usize) -> RootSection`
- `fn errored(path: &str, message: &str) -> RootSection`

- [ ] **Step 1: Read the existing `mod tests` block to confirm the imports list**

  Open `src/web/render.rs` and read the body of `mod tests` (around lines 591 to 639). Existing `use super::*;` imports the renderer's items. The fixture helpers need `Node`, `RootState`, `RootSection`, `FlaggedView`, all reachable through the existing `use super::*;` because `render.rs` already imports them at the top.

- [ ] **Step 2: Add the helpers after `extract_attr_value` (end of `mod tests`)**

  Append these helpers to the bottom of the `mod tests` block in `src/web/render.rs`, just after `extract_attr_value`:

  ```rust
  /// A directly-flagged leaf: holds audio, missing an ebook, audio filenames
  /// as given. Cover files empty.
  fn flagged_leaf(name: &str, rel: &str, audio: &[&str]) -> Node {
      Node {
          name: name.into(),
          rel_path: rel.into(),
          directly_holds_audio: true,
          missing_ebook: true,
          children: Vec::new(),
          cover_files: Vec::new(),
          audio_files: audio.iter().map(|s| (*s).into()).collect(),
      }
  }

  /// A covered leaf: holds audio AND has at least one cover file, so it is
  /// not flagged. Used by all-view tests that pin cover-file rendering.
  fn covered_leaf(name: &str, rel: &str, cover_files: &[&str]) -> Node {
      Node {
          name: name.into(),
          rel_path: rel.into(),
          directly_holds_audio: true,
          missing_ebook: false,
          children: Vec::new(),
          cover_files: cover_files.iter().map(|s| (*s).into()).collect(),
          audio_files: Vec::new(),
      }
  }

  /// A container row: no audio of its own, holds children.
  fn container(name: &str, rel: &str, children: Vec<Node>) -> Node {
      Node {
          name: name.into(),
          rel_path: rel.into(),
          directly_holds_audio: false,
          missing_ebook: false,
          children,
          cover_files: Vec::new(),
          audio_files: Vec::new(),
      }
  }

  /// Wrap a forest of root-level nodes into the `RootState::Forest` arm.
  fn forest(roots: Vec<Node>) -> RootState {
      RootState::Forest(roots)
  }

  /// One library root labeled with its display path, the given state, and
  /// the audiobook count `render_section` emits as `data-total-audiobooks`.
  fn section(path: &str, state: RootState, total: usize) -> RootSection {
      RootSection {
          path: path.into(),
          state,
          total_audiobooks: total,
      }
  }

  /// A root the scanner walked and found nothing missing in.
  fn clean(path: &str, total: usize) -> RootSection {
      section(path, RootState::Clean, total)
  }

  /// A root that errored at canonicalization or walk time.
  fn errored(path: &str, message: &str) -> RootSection {
      section(path, RootState::Error(message.into()), 0)
  }
  ```

- [ ] **Step 3: Run `cargo test --lib web::render`**

  ```bash
  cargo test --lib web::render
  ```

  Expected: the existing `single_oob_section_attribute_survives_htmx_first_colon_parse` test passes. The new helpers compile but exercise no assertion yet.

- [ ] **Step 4: Run the full suite to confirm no regression**

  ```bash
  cargo test
  ```

  Expected: same passing count as before this task.

- [ ] **Step 5: Commit**

  ```bash
  git add src/web/render.rs
  git commit -m "test(render): add synthetic FlaggedView fixture helpers

  flagged_leaf, covered_leaf, container, forest, section, clean, and errored
  build hand-crafted RootSection and Node literals next to render::tests, in
  the style of tests/curated_contract.rs. No assertions added; consumers land
  in the cluster-by-cluster migration that follows."
  ```

---

## Task 2: Migrate render cluster A (row shape and depth tags)

Move tests that pin the per-row `class="row ..."` markup that `render_node` emits. Each test today seeds a tempdir to grow a specific tree shape; the fixture replaces the seed.

**Files:**
- Modify: `src/web.rs` (delete 5 tests)
- Modify: `src/web/render.rs` (add 5 tests under `mod tests`)

**Tests to move:**

| Today's name in `web.rs::tests` | Today's seed shape | Fixture shape in `render::tests` |
|---|---|---|
| `index_renders_a_flagged_folder` | `Book/01.mp3` | one flagged leaf at top level |
| `index_tags_container_rows_by_depth` | `Author/Series/Book/01.mp3` | container(top) > container(nested) > flagged leaf |
| `index_leaves_a_deep_gap_unmarked` | `Author/Series/Book/01.mp3` | same as above, asserts no smell on the nested gap |
| `show_all_keeps_depth_tags_on_covered_containers` | mixed all-view tree | all-view fixture with covered + flagged leaves under a depth tree |
| `section_carries_a_data_root_hook` | one root, one flagged | one flagged leaf, asserts `data-root="0"` |

- [ ] **Step 1: Read each test in `src/web.rs::tests` named above**

  Note the exact `assert!(body.contains("..."))` lines per test. They become the post-migration assertions byte-for-byte. The fixture replaces the tempdir seed; nothing else changes inside the test body except the harness.

- [ ] **Step 2: Add `index_renders_a_flagged_folder` to `render::tests`**

  Append inside `mod tests` in `src/web/render.rs`:

  ```rust
  #[test]
  fn index_renders_a_flagged_folder() {
      let view = vec![section(
          "/lib",
          forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
          1,
      )];
      let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
      assert!(html.contains("Book"));
  }
  ```

- [ ] **Step 3: Add `index_tags_container_rows_by_depth` to `render::tests`**

  ```rust
  #[test]
  fn index_tags_container_rows_by_depth() {
      let view = vec![section(
          "/lib",
          forest(vec![container(
              "Author",
              "Author",
              vec![container(
                  "Series",
                  "Author/Series",
                  vec![flagged_leaf("Book", "Author/Series/Book", &["01.mp3"])],
              )],
          )]),
          1,
      )];
      let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
      assert!(html.contains(r#"class="row container-top""#));
      assert!(html.contains(r#"class="row container-nested""#));
      assert!(html.contains(r#"class="row flagged""#));
  }
  ```

- [ ] **Step 4: Add `index_leaves_a_deep_gap_unmarked` to `render::tests`**

  Same fixture as Step 3. The original test asserts the smell-label string is absent on the deeply-nested gap. Copy its `assert!(!body.contains(...))` lines verbatim, replacing `body` with `html`.

- [ ] **Step 5: Add `show_all_keeps_depth_tags_on_covered_containers` to `render::tests`**

  Read the original (around line 460 in `web.rs`) to see which paths and `.epub` files it seeds. Mirror the shape with `container` and `covered_leaf`. Call `render_view(&view, &[], ViewMode::All)`. Assertion lines copy verbatim.

- [ ] **Step 6: Add `section_carries_a_data_root_hook` to `render::tests`**

  Read the original (around line 1217). One flagged leaf, assert on `data-root="0"`. Use `render_view` or call `render_section(&view[0], 0, None, &[], ViewMode::GapsOnly)` directly. Either is fine; prefer `render_section` since the assertion is about that wrapper.

- [ ] **Step 7: Run the new tests**

  ```bash
  cargo test --lib web::render::tests
  ```

  Expected: 5 new tests pass alongside the existing OOB-pin.

- [ ] **Step 8: Delete the 5 original tests from `src/web.rs::tests`**

  Remove the `#[tokio::test] async fn ...` blocks for the 5 names above, along with their surrounding blank lines. Keep the `app_for*` and `body_string` helpers (later tasks still use them).

- [ ] **Step 9: Run the full suite**

  ```bash
  cargo test
  ```

  Expected: same passing count, with 5 fewer router-boot tests and 5 new render-level tests.

- [ ] **Step 10: Commit**

  ```bash
  git add src/web.rs src/web/render.rs
  git commit -m "refactor(tests): move row-shape and depth-tag tests onto the renderer

  Five tests in web.rs::tests reached through axum::Router to assert
  on render_node's row classes. Move them next to render.rs, calling
  render_view (and render_section for the data-root pin) directly on
  synthetic FlaggedView fixtures. Assertion lines unchanged. Drops 5
  router boots from cargo test."
  ```

---

## Task 3: Migrate render cluster B (loose / mixed badges)

`smell_label` renders the "loose at top" and "holds audio + subfolders" labels. One test today exercises both.

**Files:**
- Modify: `src/web.rs` (delete 1 test)
- Modify: `src/web/render.rs` (add 1 test)

**Tests to move:**

| Today's name | Seed shape | Fixture shape |
|---|---|---|
| `index_marks_loose_and_mixed_flagged_folders` | `The Hobbit/01.mp3` + `Terry Pratchett/01.mp3` + `Terry Pratchett/Going Postal/01.mp3` | two roots or one root with two top-level entries: a loose flagged leaf and a mixed node (flagged + has a flagged child) |

- [ ] **Step 1: Read the original `index_marks_loose_and_mixed_flagged_folders`** (around line 356)

  Note its assert lines: it asserts the body contains "loose at top" and "holds audio + subfolders".

- [ ] **Step 2: Build the fixture**

  The mixed node directly holds audio and has a child that is also a flagged leaf. Both `directly_holds_audio` and `missing_ebook` are true on the mixed parent.

  ```rust
  #[test]
  fn index_marks_loose_and_mixed_flagged_folders() {
      // A loose gap: a flagged leaf at the top of the root.
      let loose = flagged_leaf("The Hobbit", "The Hobbit", &["01.mp3"]);

      // A mixed gap: a parent that itself holds audio AND has a flagged child.
      let mixed = Node {
          name: "Terry Pratchett".into(),
          rel_path: "Terry Pratchett".into(),
          directly_holds_audio: true,
          missing_ebook: true,
          children: vec![flagged_leaf(
              "Going Postal",
              "Terry Pratchett/Going Postal",
              &["01.mp3"],
          )],
          cover_files: Vec::new(),
          audio_files: vec!["01.mp3".into()],
      };

      let view = vec![section("/lib", forest(vec![loose, mixed]), 3)];
      let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();

      assert!(html.contains("loose at top"));
      assert!(html.contains("holds audio + subfolders"));
  }
  ```

- [ ] **Step 3: Run `cargo test --lib web::render::tests::index_marks_loose_and_mixed_flagged_folders`**

  Expected: pass.

- [ ] **Step 4: Delete the original from `web.rs::tests` and run `cargo test`**

  Expected: full suite green.

- [ ] **Step 5: Commit**

  ```bash
  git add src/web.rs src/web/render.rs
  git commit -m "refactor(tests): move loose and mixed smell-label test onto the renderer"
  ```

---

## Task 4: Migrate render cluster C (file lists and counts)

`file_count` and `file_rows` emit the singular vs plural row, the collapsed `<details>` block, and the file-row markup inside a mixed node.

**Files:**
- Modify: `src/web.rs` (delete 3 tests)
- Modify: `src/web/render.rs` (add 3 tests)

**Tests to move:**

| Today's name | Seed shape | Fixture shape |
|---|---|---|
| `index_shows_a_file_count_and_a_collapsed_file_list_on_a_flagged_leaf` | `Book/01 - The Gunslinger.mp3` | one flagged leaf with one named audio file |
| `index_pluralizes_the_file_count` | `Book/01.mp3 + 02.mp3 + 03.mp3` | one flagged leaf with 3 audio files |
| `mixed_node_shows_its_own_files_above_its_child_gap` | `Pratchett/01 - Colour.mp3 + Pratchett/Going Postal/01.mp3` | mixed parent (holds audio) with a flagged-leaf child |

- [ ] **Step 1: Build the file-count test**

  ```rust
  #[test]
  fn index_shows_a_file_count_and_a_collapsed_file_list_on_a_flagged_leaf() {
      let view = vec![section(
          "/lib",
          forest(vec![flagged_leaf(
              "Book",
              "Book",
              &["01 - The Gunslinger.mp3"],
          )]),
          1,
      )];
      let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
      assert!(html.contains("1 file"));
      assert!(html.contains(r#"<details class="node-files">"#));
      assert!(html.contains("01 - The Gunslinger.mp3"));
  }
  ```

- [ ] **Step 2: Build the pluralization test**

  Three audio files; assert on `"3 files"` and the absence of `"3 file "` (trailing space pinpoints the singular row). Copy assertion lines from the original at line 399.

- [ ] **Step 3: Build the mixed-node ordering test**

  Mirror the mixed-parent shape from Task 3 but with one cover-free child and a named audio file on the parent. Assert that the parent's own file row renders and the child gap still renders. Copy assertion lines from the original at line 422.

- [ ] **Step 4: Run `cargo test --lib web::render::tests`**

  Expected: 3 new tests pass.

- [ ] **Step 5: Delete the 3 originals, run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/render.rs
  git commit -m "refactor(tests): move file-list and file-count tests onto the renderer"
  ```

---

## Task 5: Migrate render cluster D (section structure)

`render_section`'s wrapper carries the `<section class="card root" id="root-N-section" data-root="N" data-total-audiobooks="...">` opener, the `root_badge`, the empty-Forest "Nothing here" arm, the Clean arm, the Error arm, and the inline-alert arm.

**Files:**
- Modify: `src/web.rs` (delete 7 tests)
- Modify: `src/web/render.rs` (add 7 tests)

**Tests to move:**

| Today's name | Fixture shape |
|---|---|
| `index_wraps_the_sections_in_a_roots_container` | one section; assert on `<main id="roots">` wrapper from `render_view` |
| `each_root_renders_a_collapsible_summary_with_a_gap_count` | one section with N flagged leaves; assert badge text and `<details>` wrapper |
| `a_clean_root_badge_reads_no_gaps` | `clean("/lib", 0)`; assert on the Clean badge text |
| `all_view_shows_nothing_here_for_a_root_with_no_folders` | `section("/lib", forest(vec![]), 0)`; assert "Nothing here" |
| `index_shows_the_clean_message_for_a_covered_root` | `clean("/lib", 1)`; assert "No missing ebooks in this root" |
| `section_open_tag_carries_total_audiobooks_data_attr` | section with `total_audiobooks: 2`; assert `data-total-audiobooks="2"` |
| `section_open_tag_carries_zero_total_audiobooks_for_errored_root` | `errored("/lib", "...")`; assert `data-total-audiobooks="0"` |

- [ ] **Step 1: Build each of the 7 tests in order**

  Each test follows the same pattern. Example for `section_open_tag_carries_total_audiobooks_data_attr`:

  ```rust
  #[test]
  fn section_open_tag_carries_total_audiobooks_data_attr() {
      let view = vec![section(
          "/lib",
          forest(vec![
              flagged_leaf("Book A", "Book A", &["01.mp3"]),
              flagged_leaf("Book B", "Book B", &["01.mp3"]),
          ]),
          2,
      )];
      let html = render_section(&view[0], 0, None, &[], ViewMode::GapsOnly).into_string();
      assert!(html.contains(r#"data-total-audiobooks="2""#));
  }
  ```

  Example for `index_wraps_the_sections_in_a_roots_container`:

  ```rust
  #[test]
  fn index_wraps_the_sections_in_a_roots_container() {
      let view = vec![section(
          "/lib",
          forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
          1,
      )];
      let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
      assert!(html.contains(r#"<main id="roots""#));
  }
  ```

  Read each original test in `web.rs` and copy its `assert!` lines verbatim into the new test body.

- [ ] **Step 2: Run `cargo test --lib web::render::tests`**

  Expected: 7 new tests pass.

- [ ] **Step 3: Delete the 7 originals from `web.rs::tests` and run `cargo test`**

- [ ] **Step 4: Commit**

  ```bash
  git add src/web.rs src/web/render.rs
  git commit -m "refactor(tests): move section-wrapper and root-state tests onto the renderer"
  ```

---

## Task 6: Migrate render cluster E (marker buttons and action sheet)

`marker_buttons` and `row_actions` produce the per-row action set, the action-sheet popover, and the view-mode-conditional visibility. Eleven tests today; one needs splitting.

**Files:**
- Modify: `src/web.rs` (delete or split 11 tests)
- Modify: `src/web/render.rs` (add 11 tests, or 10 if `index_renders_the_marker_buttons_and_script` migrates its render-only assertions and leaves the script-tag assertion for Task 11)

**Tests to move:**

- `marker_form_delays_the_swap_only_in_gaps_only`
- `index_renders_the_marker_buttons_and_script` (render-only half; the `<script>` assertion folds into Task 11's P1)
- `elsewhere_button_uses_the_book_check_icon`
- `marker_buttons_carry_confirm_metadata`
- `all_view_dims_covered_rows_and_omits_their_buttons`
- `all_view_keeps_buttons_on_a_container_above_a_gap`
- `each_actionable_row_has_an_actions_trigger`
- `the_action_sheet_titles_with_the_folder_and_shows_verbose_labels`
- `the_action_sheet_marks_the_search_section`
- `a_covered_row_has_no_actions_trigger`
- `marking_in_all_mode_shows_the_written_marker_on_the_row`

Most tests need a single flagged leaf and assert on the action-row markup. `marking_in_all_mode_shows_the_written_marker_on_the_row` needs a covered leaf with a `.no_ebook` cover file (covered because the marker covers it), and exercises the all-view code path.

- [ ] **Step 1: For each test, read the original in `web.rs` to extract its assertion lines and the data shape**

- [ ] **Step 2: Build each fixture and migrated test**

  Worked example for `each_actionable_row_has_an_actions_trigger`:

  ```rust
  #[test]
  fn each_actionable_row_has_an_actions_trigger() {
      let view = vec![section(
          "/lib",
          forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
          1,
      )];
      let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
      // Copy the original assert lines verbatim.
      assert!(html.contains(r#"class="row-actions-trigger""#));
  }
  ```

  Worked example for `marking_in_all_mode_shows_the_written_marker_on_the_row`:

  ```rust
  #[test]
  fn marking_in_all_mode_shows_the_written_marker_on_the_row() {
      let view = vec![section(
          "/lib",
          forest(vec![covered_leaf("Book", "Book", &[".no_ebook"])]),
          1,
      )];
      let html = render_view(&view, &[], ViewMode::All).into_string();
      // Copy the original assert lines verbatim.
      // The original asserts on the marker badge text and class.
  }
  ```

  For `index_renders_the_marker_buttons_and_script`: copy only the marker-button assertions; do not copy the `<script src="...">` assertion (that lives in Task 11, P1).

- [ ] **Step 3: Run `cargo test --lib web::render::tests`**

- [ ] **Step 4: Delete the originals (or the render-portion of the split test)**

- [ ] **Step 5: Run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/render.rs
  git commit -m "refactor(tests): move marker-button and action-sheet tests onto the renderer

  index_renders_the_marker_buttons_and_script splits: marker-button
  assertions land in render::tests; the body-end <script> assertion is
  picked up by page::tests in a later commit."
  ```

---

## Task 7: Migrate render cluster F (search links)

`search_links` emits the per-row search-link popover. Five tests today.

**Files:**
- Modify: `src/web.rs` (delete 5 tests)
- Modify: `src/web/render.rs` (add 5 tests)

**Tests to move:**

- `index_renders_the_search_links`
- `index_renders_every_configured_link`
- `index_omits_the_links_span_when_none_are_configured`
- `search_links_render_inside_a_popover_menu`
- `search_link_query_percent_encodes_spaces`

These tests need a `&[SearchLink]` slice (not `&[]`). `SearchLink` lives in `crate::config`; it's reachable from `render::tests` via `use crate::config::SearchLink;` (the existing `mod tests` already does `use super::*;` and the crate path resolves through the `super::*` import chain; if it does not, add the explicit `use crate::config::SearchLink;`).

- [ ] **Step 1: Add `use crate::config::SearchLink;` to `render::tests` if not already imported**

- [ ] **Step 2: Build each fixture**

  Worked example for `index_renders_the_search_links`:

  ```rust
  #[test]
  fn index_renders_the_search_links() {
      let links = vec![SearchLink {
          name: "Goodreads".into(),
          url_template: "https://goodreads.com/search?q={query}".into(),
      }];
      let view = vec![section(
          "/lib",
          forest(vec![flagged_leaf("Dune", "Dune", &["01.mp3"])]),
          1,
      )];
      let html = render_view(&view, &links, ViewMode::GapsOnly).into_string();
      assert!(html.contains("Goodreads"));
  }
  ```

  Worked example for `index_omits_the_links_span_when_none_are_configured`: pass `&[]`. Copy the negative assertion (`assert!(!html.contains("..."))`) from the original.

  Worked example for `search_link_query_percent_encodes_spaces`: use a flagged leaf with a multi-word name (`"The Stand"`); assert the rendered URL contains `The%20Stand` or similar. Copy the original assertion.

- [ ] **Step 3: Run `cargo test --lib web::render::tests`**

- [ ] **Step 4: Delete the originals, run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/render.rs
  git commit -m "refactor(tests): move search-link tests onto the renderer"
  ```

---

## Task 8: Migrate render cluster G (cover files and status icons)

`cover_files_span` and `status_icon` differ between gaps-only and all-view. Four tests.

**Files:**
- Modify: `src/web.rs` (delete 4 tests)
- Modify: `src/web/render.rs` (add 4 tests)

**Tests to move:**

- `all_view_lists_the_covering_ebook_on_a_covered_row`
- `gaps_only_view_lists_no_cover_files`
- `gaps_only_view_has_no_status_icons_or_covered_rows`
- `all_view_renders_covered_folders_that_gaps_only_drops`

All four call `render_view` with one root carrying a mix of flagged and covered leaves. They differ only in the `ViewMode` and the assertion focus.

- [ ] **Step 1: Build each fixture**

  Worked example for `all_view_lists_the_covering_ebook_on_a_covered_row`:

  ```rust
  #[test]
  fn all_view_lists_the_covering_ebook_on_a_covered_row() {
      let view = vec![section(
          "/lib",
          forest(vec![covered_leaf("Dune", "Dune", &["Dune.epub"])]),
          1,
      )];
      let html = render_view(&view, &[], ViewMode::All).into_string();
      assert!(html.contains("Dune.epub"));
  }
  ```

- [ ] **Step 2: Run `cargo test --lib web::render::tests`**

- [ ] **Step 3: Delete the originals, run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/render.rs
  git commit -m "refactor(tests): move cover-file and status-icon tests onto the renderer"
  ```

---

## Task 9: Migrate render cluster H (gap summary strip)

`gap_summary`, `coverage_bar`, and `root_chip` build the strip above the root list. Ten tests cover initial paint, all-clear states, error handling, multi-root chips, and the progressbar.

**Files:**
- Modify: `src/web.rs` (delete 10 tests)
- Modify: `src/web/render.rs` (add 10 tests)

**Tests to move:**

- `index_renders_the_gap_summary_strip`
- `gap_summary_initial_paint_carries_library_coverage_readout`
- `gap_summary_all_clear_with_audiobooks_shows_trailing_coverage_fragment`
- `gap_summary_empty_library_keeps_coverage_fragment_hidden`
- `gap_summary_excludes_errored_roots_from_the_coverage_total`
- `gap_summary_shows_all_clear_for_a_covered_library`
- `gap_summary_renders_a_chip_per_root_for_a_multi_root_config`
- `gap_summary_omits_chips_for_a_single_root`
- `gap_summary_chips_handle_a_clean_and_an_error_root`
- `gap_summary_renders_a_library_coverage_progressbar`

Some need multi-root fixtures. The errored-root tests use `errored("/lib", "io error")`. The all-clear tests use `clean("/lib", N)`.

- [ ] **Step 1: Build each fixture**

  Worked example for `gap_summary_renders_a_chip_per_root_for_a_multi_root_config`:

  ```rust
  #[test]
  fn gap_summary_renders_a_chip_per_root_for_a_multi_root_config() {
      let view = vec![
          section(
              "/lib-a",
              forest(vec![flagged_leaf("A", "A", &["01.mp3"])]),
              1,
          ),
          clean("/lib-b", 2),
      ];
      let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
      // Copy the original assertions.
  }
  ```

  Worked example for `gap_summary_excludes_errored_roots_from_the_coverage_total`:

  ```rust
  #[test]
  fn gap_summary_excludes_errored_roots_from_the_coverage_total() {
      let view = vec![
          section(
              "/lib",
              forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
              1,
          ),
          errored("/missing", "no such file or directory"),
      ];
      let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
      // Copy the original assertions (errored total does not contribute to coverage).
  }
  ```

- [ ] **Step 2: Run `cargo test --lib web::render::tests`**

- [ ] **Step 3: Delete the originals, run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/render.rs
  git commit -m "refactor(tests): move gap-summary strip tests onto the renderer"
  ```

---

## Task 10: Add `mod tests` to `src/web/page.rs`

The page module has no test block today. Set up the scaffolding before the page clusters land.

**Files:**
- Modify: `src/web/page.rs`

- [ ] **Step 1: Append `#[cfg(test)] mod tests` at the bottom of `src/web/page.rs`**

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use maud::html;

      // A stub body for tests that only care about the page shell, not what
      // sits inside it.
      fn stub_body() -> maud::Markup {
          html! { div #stub {} }
      }
  }
  ```

  `use super::*;` imports `ViewMode`, `FAVICON_HREF`, and every `pub(crate)` helper in the module.

- [ ] **Step 2: Run `cargo test --lib web::page`**

  Expected: zero tests reported (the module compiles but has no assertions).

- [ ] **Step 3: Commit**

  ```bash
  git add src/web/page.rs
  git commit -m "test(page): scaffold mod tests with a stub_body helper

  Future page-cluster migrations land their assertions here, next to the
  page-shell builders they test."
  ```

---

## Task 11: Migrate page cluster P1 (head and shell)

Head-block and body-end assertions: favicon, prepaint accent bootstrap, stylesheet link, noscript notice, body-end script tags. Four tests today, plus the body-end `<script>` assertion from `index_renders_the_marker_buttons_and_script` (split during Task 6).

**Files:**
- Modify: `src/web.rs` (delete 4 tests; remove the script assertion stranded from Task 6 if it survived)
- Modify: `src/web/page.rs` (add 4 or 5 tests)

**Tests to move:**

- `index_links_an_inline_favicon`
- `index_links_the_stylesheet_and_inits_the_theme`
- `prepaint_bootstrap_handles_the_accent_preference`
- `page_carries_a_noscript_notice`
- (the body-end `<script>` half of `index_renders_the_marker_buttons_and_script`, if it was not folded into one of the four above)

- [ ] **Step 1: Add each test, calling `page::page(ViewMode::GapsOnly, stub_body())`**

  Worked example for `index_links_an_inline_favicon`:

  ```rust
  #[test]
  fn index_links_an_inline_favicon() {
      let html = page(ViewMode::GapsOnly, stub_body()).into_string();
      assert!(html.contains(r#"rel="icon""#));
      assert!(html.contains(r#"href="/static/brand/favicon.svg""#)); // or the existing FAVICON_HREF literal
  }
  ```

  Copy the original assertion lines from the corresponding test in `web.rs`.

  Worked example for `page_carries_a_noscript_notice`:

  ```rust
  #[test]
  fn page_carries_a_noscript_notice() {
      let html = page(ViewMode::GapsOnly, stub_body()).into_string();
      assert!(html.contains("<noscript"));
      assert!(html.contains("Missing Ebooks needs JavaScript to run"));
  }
  ```

- [ ] **Step 2: If `index_renders_the_marker_buttons_and_script` left a stranded script-tag assertion, add it to P1**

  ```rust
  #[test]
  fn page_loads_htmx_htmx_sse_and_app_scripts() {
      let html = page(ViewMode::GapsOnly, stub_body()).into_string();
      assert!(html.contains(r#"src="/static/htmx.min.js""#));
      assert!(html.contains(r#"src="/static/htmx-sse.js""#));
      assert!(html.contains(r#"src="/static/app.js""#));
  }
  ```

- [ ] **Step 3: Run `cargo test --lib web::page::tests`**

- [ ] **Step 4: Delete the originals from `web.rs::tests`, run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/page.rs
  git commit -m "refactor(tests): move head-block and script-tag tests onto the page shell"
  ```

---

## Task 12: Migrate page cluster P2 (navbar and indicators)

The largest page cluster: nine navbar tests plus the rescan-form half and the scan-bar half of `rescan_is_an_in_place_htmx_swap_with_a_progress_bar`.

**Files:**
- Modify: `src/web.rs` (delete 9 tests + the rescan-form test split)
- Modify: `src/web/page.rs` (add 10 or 11 tests)

**Tests to move:**

- `navbar_renders_a_settings_cog_with_theme_and_confirm_controls`
- `panel_renders_the_accent_color_control`
- `the_view_control_marks_the_active_segment`
- `index_renders_the_menu_with_a_flagged_badge`
- `the_flagged_badge_carries_a_hover_title`
- `navbar_renders_the_brand_mark_before_the_title`
- `decorative_icons_are_hidden_from_assistive_tech`
- `navbar_places_the_spacer_before_the_search_box`
- `index_renders_the_shortcuts_inside_the_settings_panel`
- `rescan_is_an_in_place_htmx_swap_with_a_progress_bar`

`scan_bar()` is a pub(crate) helper, callable directly:

```rust
#[test]
fn scan_bar_carries_the_indicator_id() {
    let html = scan_bar().into_string();
    assert!(html.contains(r#"id="scan-bar""#));
}
```

Navbar tests call `page(ViewMode::GapsOnly, stub_body())` and assert on navbar substrings. View-toggle tests vary `ViewMode` and assert on the active-segment marker:

```rust
#[test]
fn the_view_control_marks_the_active_segment_in_gaps_only() {
    let html = page(ViewMode::GapsOnly, stub_body()).into_string();
    // Copy the original assertions.
}
```

For tests that today asserted both gaps-only and all-view active segments, either split into two `#[test]` cases (one per `ViewMode`) or keep one test and call `page(...)` twice. Either way is fine; split if it makes the assertions clearer.

`rescan_is_an_in_place_htmx_swap_with_a_progress_bar` migrates as:

```rust
#[test]
fn navbar_renders_the_rescan_form_with_htmx_attrs() {
    let html = page(ViewMode::GapsOnly, stub_body()).into_string();
    assert!(html.contains(r#"hx-post="/rescan""#));
    assert!(html.contains(r##"hx-target="#roots""##));
    assert!(html.contains(r##"hx-indicator="#scan-bar, #rescan-btn""##));
    assert!(html.contains(r##"hx-disabled-elt="#rescan-btn""##));
    assert!(html.contains(r#"id="rescan-btn""#));
    assert!(html.contains("Rescan"));
    assert!(!html.contains(r#"action="/rescan""#));
    assert!(!html.contains(r#"method="post""#));
    assert!(html.contains(r#"id="rescan-btn" type="button" hx-post="/rescan""#));
}
```

- [ ] **Step 1: Add each test to `page::tests`**

- [ ] **Step 2: Run `cargo test --lib web::page::tests`**

- [ ] **Step 3: Delete the 10 originals from `web.rs::tests`, run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/page.rs
  git commit -m "refactor(tests): move navbar, view-toggle, settings-menu, and rescan-form tests onto the page shell

  rescan_is_an_in_place_htmx_swap_with_a_progress_bar splits: the
  navbar's rescan-form attrs land here as navbar_renders_the_rescan_form_with_htmx_attrs;
  the scan-bar id pin lands here as scan_bar_carries_the_indicator_id."
  ```

---

## Task 13: Migrate page cluster P3 (search box)

`search_box()` is callable directly. Two tests.

**Files:**
- Modify: `src/web.rs` (delete 2 tests)
- Modify: `src/web/page.rs` (add 2 tests)

**Tests to move:**

- `navbar_renders_the_disabled_filter_input_and_no_matches_line`
- `search_box_renders_a_hidden_themed_clear_button`

- [ ] **Step 1: Add each test**

  Worked example:

  ```rust
  #[test]
  fn search_box_renders_a_hidden_themed_clear_button() {
      let html = search_box().into_string();
      // Copy the original assertions.
  }
  ```

  Use `search_box()` directly. The `no_matches` assertion can call `search_empty()` for the no-matches line if that string is its responsibility.

- [ ] **Step 2: Run `cargo test --lib web::page::tests`, delete originals, run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/page.rs
  git commit -m "refactor(tests): move search-box tests onto the page shell"
  ```

---

## Task 14: Migrate page cluster P4 (stack: banner, dialog, toast)

`conn_banner()`, `confirm_dialog()`, and `toast()` are callable directly. Three tests.

**Files:**
- Modify: `src/web.rs` (delete 3 tests)
- Modify: `src/web/page.rs` (add 3 tests)

**Tests to move:**

- `index_renders_the_hidden_connection_banner`
- `index_renders_the_confirm_dialog`
- `index_renders_the_toast_stack_and_template`

- [ ] **Step 1: Add each test**

  ```rust
  #[test]
  fn conn_banner_is_hidden_by_default() {
      let html = conn_banner().into_string();
      // Copy the original assertions.
  }

  #[test]
  fn confirm_dialog_renders_its_buttons_and_message_slot() {
      let html = confirm_dialog().into_string();
      // Copy the original assertions.
  }

  #[test]
  fn toast_renders_a_stack_and_a_template() {
      let html = toast().into_string();
      // Copy the original assertions.
  }
  ```

- [ ] **Step 2: Run `cargo test --lib web::page::tests`, delete originals, run `cargo test`, commit**

  ```bash
  git add src/web.rs src/web/page.rs
  git commit -m "refactor(tests): move connection-banner, confirm-dialog, and toast tests onto the page shell"
  ```

---

## Task 15: Trim `web.rs::tests` and drop unused helpers

After 13 migration commits, `web.rs::tests` carries only the handler-shape tests (mark / unmark / rescan / static assets / 304 / query-param tolerance). Some test helpers may now be unused.

**Files:**
- Modify: `src/web.rs`

- [ ] **Step 1: Run `cargo test`**

  Confirm green before any deletion.

- [ ] **Step 2: Check helper usage**

  ```bash
  rg "app_for_with_links|app_for_roots" src/web.rs
  ```

  If either helper has zero call sites in the remaining test block, delete it. Likewise check `body_string` and `app_for`; both will likely still be used by the handler-shape tests that seed a tempdir to verify on-disk marker side effects.

- [ ] **Step 3: Run `cargo clippy --all-targets`**

  Catch any `#[allow(dead_code)]`-style warnings on now-unused imports or helpers.

- [ ] **Step 4: Tidy imports**

  Remove any `use` lines no longer referenced by the remaining handler-shape tests.

- [ ] **Step 5: Run `cargo test`**

  Expected: green, with a noticeably shorter `web.rs` (target: roughly 15 to 17 `#[tokio::test]` cases, down from 82).

- [ ] **Step 6: Commit**

  ```bash
  git add src/web.rs
  git commit -m "chore(tests): drop helpers stranded by the render and page test migration

  app_for_with_links and app_for_roots have no callers after the markup-shape
  tests moved onto render.rs and page.rs. body_string and app_for stay; the
  remaining handler-shape tests still seed a tempdir to verify the on-disk
  marker side effects."
  ```

---

## Self-review checklist

After Task 15 lands, run through this list:

- [ ] `cargo test` is green.
- [ ] `src/web.rs` is roughly half its original line count (target: under 1100 lines).
- [ ] `src/web/render.rs` `mod tests` carries roughly 45 tests plus the original OOB pin.
- [ ] `src/web/page.rs` `mod tests` carries roughly 18 tests.
- [ ] `src/web.rs::tests` carries roughly 15 to 17 `#[tokio::test]` cases, all handler-shape.
- [ ] `git log --oneline` since this plan started shows 14 to 15 commits, each on a coherent cluster, none failing on its own.
- [ ] `tests/cache_render_byte_equal.rs` and `tests/curated_contract.rs` are unchanged.
- [ ] No new ADR. No production code touched.

## References

- Spec: `.scratch/render-page-tests/spec.md`
- Architecture review findings: `.scratch/architecture-review/findings.md` (candidate #2, the top recommendation)
- Existing template for synthetic `Node` fixtures: `tests/curated_contract.rs`
- Existing template for direct renderer calls: `tests/cache_render_byte_equal.rs`
- Code-comments style skill: `~/.claude/skills/writing-style-code-comments/SKILL.md`
