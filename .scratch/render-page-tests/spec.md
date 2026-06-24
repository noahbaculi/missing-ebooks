# Render and page tests: move markup assertions off the router

Architecture-review candidate #2. Relocate the markup-shape tests in `src/web.rs::tests` onto `src/web/render.rs::tests` and `src/web/page.rs::tests`. The router test surface shrinks to the handler-shape concerns it owns. The deep modules (`render`, `page`) get the test surface their interfaces deserve. No production code changes, no ADR conflict.

## Background

`src/web.rs` is 2166 lines: roughly 280 production and 1886 test. The `mod tests` block holds 82 `#[tokio::test]` cases. Each boots an axum `Router`, fires a `Request` through `oneshot`, awaits `response.into_body().collect()`, decodes the bytes to a `String`, and asserts on substrings like `body.contains(r#"class="row container-top""#)` or `body.contains("Goodreads")` or `body.contains(r#"data-total-audiobooks="2""#)`. The router is acting as an adapter to the renderer.

The renderer (`src/web/render.rs`, 640 lines) and the page shell (`src/web/page.rs`, 309 lines) are the deep modules with the markup. `render.rs` carries one colocated test (the htmx OOB attribute pin) plus a small `extract_attr_value` helper. `page.rs` has no test module.

Two integration tests under `tests/` already show the direct shape: `tests/curated_contract.rs` hand-builds `ScannedFolder` and `Node` literals; `tests/cache_render_byte_equal.rs` calls `render_section` and `oob_sections` directly on a `FlaggedView`.

## Goal

Three test surfaces, each scoped to one module's interface:

- `src/web/render.rs::tests` exercises `render_section`, `roots`, `render_view`, `single_oob_section`, `oob_sections` on hand-built `FlaggedView` fixtures.
- `src/web/page.rs::tests` exercises `page::page` and its helpers (`view_toggle`, `settings_menu`, `search_box`, `conn_banner`, `confirm_dialog`, `toast`, `scan_bar`, `search_empty`).
- `src/web.rs::tests` keeps only handler-shape tests: status codes, `HX-Push-Url`, `HX-Trigger`, `Set-Cookie`, the `failed_write_response` re-fetch path, conditional-request semantics on the static-asset routes.

After the migration `web.rs` shrinks toward its real depth, and a markup regression fails colocated with the module that owns the markup.

## Production surface

No production code changes. The visibility already permits colocated tests:

- `render_section`, `single_oob_section`, `oob_sections` are `pub`.
- `roots`, `render_view` are `pub(crate)` and visible inside `render::tests`.
- `page::page` and every shell helper (`view_toggle`, `settings_menu`, `search_box`, `conn_banner`, `confirm_dialog`, `toast`, `scan_bar`, `search_empty`) are `pub(crate)` and visible inside `page::tests`.
- `RootSection`, `FlaggedView`, `RootState`, `Node`, `ViewMode`, `SearchLink`, `Marker` are all `pub` already (used by `tests/cache_render_byte_equal.rs`).

## Fixture style

Hand-constructed types, not tempdir + scan. Each test builds the smallest `FlaggedView` that exercises its assertion. `tests/curated_contract.rs` is the existing template for synthetic `Node` literals; this work generalizes the pattern to `RootSection` and `FlaggedView`.

A small fixture block sits at the top of `render::tests`:

```rust
// One leaf row that directly holds audio and is missing an ebook.
fn flagged_leaf(name: &str, rel: &str, audio: &[&str]) -> Node { ... }

// A container row with children, no audio of its own.
fn container(name: &str, rel: &str, children: Vec<Node>) -> Node { ... }

// Wrap a forest into a RootSection. `total` is data-total-audiobooks.
fn section(path: &str, state: RootState, total: usize) -> RootSection { ... }

// A clean root: RootSection { state: RootState::Clean, total_audiobooks: total, ... }.
fn clean(path: &str, total: usize) -> RootSection { ... }

// An errored root: RootSection { state: RootState::Error(message.into()), total_audiobooks: 0, ... }.
fn errored(path: &str, message: &str) -> RootSection { ... }
```

`page::tests` does not need a `FlaggedView` for most assertions. Its tests call `page::page(ViewMode::GapsOnly, html! { "stub body" })` or call the shell helpers directly. Where a page-level assertion needs a body (the rescan form's `hx-target="#roots"` lives in the navbar, not the body), the stub body is enough.

## Test categorization

The 82 tests in `web.rs::tests` split three ways. Cluster counts are approximate; the plan pins exact destinations per test.

### Markup-shape clusters (destination: `src/web/render.rs::tests`)

| Cluster | What it pins | Renderer surface |
|---|---|---|
| A. Row shape and depth | Container-top, container-nested, flagged-leaf classes by depth | `render_node` |
| B. Loose / mixed badges | `smell_label` output for loose-at-top and holds-audio-plus-subfolders | `smell_label` |
| C. File lists and counts | Singular vs plural file count, collapsed `<details>` file list, mixed-node ordering | `file_count`, `file_rows` |
| D. Section structure | `<section>` wrapper, `data-root` and `data-total-audiobooks` attrs, root-badge for Forest / Clean / Error, the empty-Forest "Nothing here" arm, the inline-alert arm, the scan-bar wrapper | `render_section`, `root_badge` |
| E. Marker buttons and action sheet | Button visibility per view mode, confirm metadata, action-sheet title and labels, search-section row, covered-row suppression, marker-delay class in gaps-only, written marker badge on a row after a mark | `marker_buttons`, `row_actions` |
| F. Search links | One link, every configured link, omitted-when-none, popover menu shape, percent-encoded query | `search_links` |
| G. Cover files and status icons | All-view lists the covering ebook, gaps-only suppresses cover files and status icons, all-view dims covered rows | `cover_files_span`, `status_icon` |
| H. Gap summary strip | Initial-paint library-coverage readout, all-clear trailing fragment, empty-library hidden fragment, errored roots excluded, all-clear for covered, chip per root for multi-root, no chips for single-root, chip handling for clean and error, library-coverage progressbar | `gap_summary`, `coverage_bar`, `root_chip` |

### Page-shell clusters (destination: `src/web/page.rs::tests`, new module)

| Cluster | What it pins | Page surface |
|---|---|---|
| P1. Head and shell | Inline favicon, prepaint accent bootstrap, stylesheet link, noscript notice, body-end script tags (htmx, htmx-sse, app.js) | `page::page` head and body-end blocks |
| P2. Navbar | Brand mark, spacer placement, view toggle active segment, settings cog and theme/confirm controls, accent picker panel, flagged-badge with hover title, decorative-icon `aria-hidden`, shortcuts panel inside settings, rescan form attrs (`hx-target="#roots"`, `hx-post="/rescan"`, `hx-disabled-elt`) | `page::page` navbar, `view_toggle`, `settings_menu` |
| P3. Search box | Disabled filter input with no-matches line, hidden themed clear button | `search_box` |
| P4. Stack | Hidden connection banner, confirm dialog, toast stack and template | `conn_banner`, `confirm_dialog`, `toast` |

### Handler-shape (destination: stays in `src/web.rs::tests`)

| Cluster | What it pins | Handler surface |
|---|---|---|
| Mark / unmark | POST writes the marker file on disk and returns a section response; `HX-Trigger` fires on create and stays silent on remark; DELETE removes the file and swaps back; mark-failure goes through `failed_write_response` and returns the tree-shaped re-fetch | `web::mark`, `web::unmark`, `failed_write_response` |
| Rescan | `HX-Request` header routes to the partial, no header routes to the full page | `web::rescan` |
| Static assets and 304 | Stylesheet / htmx.min.js / app.js route, `Cache-Control` and strong `ETag`, htmx finite-window not-immutable, matching/non-matching/star `If-None-Match` | `web::assets` handlers |
| Query param tolerance | Index accepts and ignores an unknown filter query param on a view switch | `web::index` |

### Two tests need splitting

`index_renders_the_marker_buttons_and_script`: the marker-button assertion belongs in render cluster E. The `<script>` tag assertion folds into page cluster P1, which pins the head and body-end script tags wholesale.

`rescan_is_an_in_place_htmx_swap_with_a_progress_bar`: the rescan form attrs (`hx-target`, `hx-post`, `hx-disabled-elt`) sit in `page::page`'s navbar, so the form-attr part goes to page cluster P2. The `#scan-bar` indicator and the progress-bar markup live in `render_view`, so the scan-bar shape goes to render cluster D. No handler-shape part survives in `web.rs`.

## Migration shape

Cluster-by-cluster, render first then page, each cluster a single commit:

1. Setup: add fixture helpers in `render::tests`. No new assertions.
2. Render clusters A through H, one commit each (8 commits).
3. Page setup: add `#[cfg(test)] mod tests` to `page.rs`.
4. Page clusters P1 through P4, one commit each (4 commits).
5. Cleanup: trim `web.rs::tests` helpers (`app_for_with_links`, `app_for_roots`, possibly `body_string`) once the remaining handler tests no longer need them.

Total: 14 to 15 commits. Each one passes `cargo test` on its own, and a bisect against any later regression points at the cluster that introduced it.

Within a cluster, every test moves in the same commit and the originals delete the same commit. No duplicate assertions on the same property survive across the migration.

## Assertion preservation

Each migrated test keeps its existing assertion strings byte-for-byte where the substring is a renderer output. Where today's test uses a router-side helper (`body_string(response).await`), the call gets replaced by `<helper>(...).into_string()`. Examples:

```rust
// before (in src/web.rs::tests):
let body = body_string(
    app_for(dir.path())
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap(),
).await;
assert!(body.contains(r#"class="row container-top""#));

// after (in src/web/render.rs::tests):
let view = vec![section(
    "/A",
    RootState::Forest(vec![container("Author", "Author", vec![
        container("Series", "Author/Series", vec![
            flagged_leaf("Book", "Author/Series/Book", &["01.mp3"]),
        ]),
    ])]),
    1,
)];
let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
assert!(html.contains(r#"class="row container-top""#));
```

A test that today asserts on a body produced by the full page (head + body) stays on `render_view` when its assertion is a body-only substring, or moves to `page::tests` when its assertion is a head/shell substring. The pluralization, depth-class, smell-label, file-count, and gap-summary assertions are body-only and land on `render::tests`. The favicon, stylesheet link, noscript notice, navbar, dialog, toast, and banner assertions are shell substrings and land on `page::tests`.

## What stays at the router level

After the migration, `web.rs::tests` carries roughly 15 to 17 tests. Each one exercises a property that the renderer interface does not express:

- Response status (200, 4xx, 304).
- Response headers (`HX-Trigger`, `Set-Cookie`, `Cache-Control`, `ETag`, `Content-Type`, the htmx finite-cache window).
- Branching on the `HX-Request` header (rescan partial vs full page).
- The on-disk side effect of `mark` and `unmark`, asserted by checking the marker file exists or does not exist on the tempdir.
- The `failed_write_response` re-fetch path: when `write_mark` errors, the handler still returns a tree-shaped body to the caller. The body content is asserted only at the shape level (a `<section>` wrapper is present), not at the markup-substring level.
- Conditional-request semantics on `/static/*`: matching / non-matching / star `If-None-Match`, with the 304 returning no body.

The `app_for_with_links` and `app_for_roots` helpers drop. `app_for` and `body_string` likely stay because the kept tests still seed a tempdir to verify the on-disk marker side effect.

## Out of scope

- `tests/curated_contract.rs`, `tests/cache_render_byte_equal.rs`, `tests/sse.rs`: integration tests that already test the right surfaces, unchanged here.
- `web::assets::tests`: the JS / CSS wire-contract pins and the `if_none_match_hit` unit tests stay where they are. Only the router-level 304 tests in `web.rs::tests` (which exercise the asset handlers' response, not the helper) are in scope, and they stay in `web.rs::tests` as handler-shape.
- `tests/accent/`: out of scope.
- The `mod tests` blocks in every other module (`scanner`, `tree`, `state`, `service`, `autosync`, `config`, `query`, `marker`, `telemetry`, `scenarios`, `demo::*`): out of scope.
- Production code in `render`, `page`, `web`, `state`, `service`, `scanner`, `tree`: out of scope. No production change ships with this work.

## ADR

None. ADR-0009, ADR-0023, ADR-0024 pin htmx-swap behaviors; the migrated tests continue to pin the same behaviors at the renderer level. No ADR contradiction.
