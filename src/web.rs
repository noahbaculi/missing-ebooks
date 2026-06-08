//! axum router, request handlers, and Maud markup. Handlers are thin: they call a
//! `service` operation and render. Handlers return `Html<String>` so Maud stays
//! decoupled from the axum version. Marker writes use htmx to swap just the
//! affected root's section; the script is vendored and served from `/static`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::header;
use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::{get, post};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde::Deserialize;

use crate::config::SearchLink;
use crate::marker::Marker;
use crate::query::clean_query;
use crate::service::{self, FlaggedView, RootSection, RootState, ViewMode};
use crate::state::AppState;
use crate::tree::Node;

/// The vendored htmx runtime, embedded at compile time and served from `/static`.
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

/// The hand-rolled stylesheet, embedded at compile time and served from `/static`.
const APP_CSS: &str = include_str!("../assets/app.css");

/// Pre-paint theme bootstrap: sets `data-theme` on <html> before first paint so
/// there is no flash, and defines `toggleTheme` for the navbar button. Saved
/// choice wins over the OS preference; the choice persists in localStorage.
const THEME_INIT_JS: &str = r#"(function () {
  var KEY = 'theme';
  function apply(t) { document.documentElement.dataset.theme = t; }
  function preferred() {
    var saved = localStorage.getItem(KEY);
    if (saved === 'light' || saved === 'dark') return saved;
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }
  apply(preferred());
  window.toggleTheme = function () {
    var next = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
    localStorage.setItem(KEY, next);
    apply(next);
  };
})();"#;

/// Half-filled circle marking the light/dark toggle. Inherits `currentColor`.
const TOGGLE_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="currentColor"><path d="M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zm0 2v14a7 7 0 0 1 0-14z"/></svg>"##;

/// Caret that rotates open when its folder is expanded. Inherits `currentColor`.
const CHEVRON_SVG: &str = r##"<svg class="chev" viewBox="0 0 16 16" fill="currentColor"><path d="M6 4l4 4-4 4z"/></svg>"##;

/// Magnifying glass for the search-links dropdown trigger. Inherits `currentColor`.
const SEARCH_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.35-4.35"/></svg>"##;

/// Folder glyph shown on every node row. Inherits `currentColor`.
const FOLDER_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"##;

/// Check mark for the "no gaps in this root" state. Inherits `currentColor`.
const CHECK_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M5 13l4 4L19 7"/></svg>"##;

/// Circled exclamation for a scan or write error. Inherits `currentColor`.
const ERROR_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M12 8v4M12 16h.01"/></svg>"##;

/// A small book glyph as an inline SVG data URI, used for the favicon so the tab
/// has an identity and the browser stops requesting `/favicon.ico`. The stroke
/// is the light-theme primary, which reads on both light and dark tab strips.
const FAVICON_HREF: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23605dff' stroke-width='2' stroke-linejoin='round'%3E%3Cpath d='M4 19V5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2z'/%3E%3Cpath d='M6 21h13'/%3E%3C/svg%3E";

/// Vertical three-dot "more actions" glyph for the mobile per-row menu trigger.
/// Inherits `currentColor`.
const KEBAB_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="12" cy="19" r="2"/></svg>"##;

/// A "no entry" sign (circle with a horizontal bar) for the sheet's "Mark as
/// None" row. Shown only inside the mobile sheet. Inherits `currentColor`.
const NO_ENTRY_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M7 12h10"/></svg>"##;

/// A book with a small check, marking that this audiobook's ebook is accounted
/// for somewhere else rather than missing. Shown on the sheet's "Ebook
/// elsewhere" button. Inherits `currentColor`.
const EBOOK_ELSEWHERE_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H19a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1H6.5a1 1 0 0 1 0-5H20"/><path d="m9 9.5 2 2 4-4"/></svg>"##;

/// The `view` parameter shared by the index query string and the rescan form. A
/// lenient `Option<String>` so an absent or unknown value falls back to gaps-only
/// rather than rejecting the request.
#[derive(Deserialize)]
struct ViewQuery {
    #[serde(default)]
    view: Option<String>,
}

/// The body of a marker write: which root, which folder, which marker, and which
/// view the click came from (so the swapped section comes back in that mode).
#[derive(Deserialize)]
struct MarkRequest {
    root: usize,
    rel: String,
    kind: Marker,
    #[serde(default)]
    view: ViewMode,
}

/// Build the application router with the shared state attached.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/mark", post(mark))
        .route("/rescan", post(rescan))
        .route("/static/htmx.min.js", get(htmx_script))
        .route("/static/app.css", get(app_css))
        .with_state(state)
}

async fn index(State(state): State<Arc<AppState>>, Query(query): Query<ViewQuery>) -> Html<String> {
    let mode = ViewMode::from_query(query.view.as_deref());
    let view = service::current_view(&state, mode).await;
    Html(page(&view, &state.config.search_links, mode).into_string())
}

async fn mark(State(state): State<Arc<AppState>>, Form(req): Form<MarkRequest>) -> Html<String> {
    let links = &state.config.search_links;
    let mode = req.view;
    match service::mark(&state, req.root, &req.rel, req.kind, mode).await {
        Ok(view) => {
            Html(render_section(&view[req.root], req.root, None, links, mode).into_string())
        }
        Err(err) => {
            let message = format!("Could not mark {}: {err}", req.rel);
            let view = service::current_view(&state, mode).await;
            let markup = match view.get(req.root) {
                Some(section) => render_section(section, req.root, Some(&message), links, mode),
                None => html! {
                    section.card.root {
                        div.alert.alert-error { (PreEscaped(ERROR_SVG)) span { (message) } }
                    }
                },
            };
            Html(markup.into_string())
        }
    }
}

async fn rescan(State(state): State<Arc<AppState>>, Form(query): Form<ViewQuery>) -> Redirect {
    let mode = ViewMode::from_query(query.view.as_deref());
    service::rescan(&state, mode).await;
    // 303 See Other: Post/Redirect/Get, so a refresh does not re-trigger a scan.
    Redirect::to(mode_path(mode))
}

/// The path that renders a given mode, for redirects and links.
fn mode_path(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::GapsOnly => "/",
        ViewMode::All => "/?view=all",
    }
}

/// The gaps-only / show-all view control for the navbar: a two-segment control.
/// The segment for the current view is inert and marked `aria-current`; the other
/// is a GET link that navigates to its view. Switching reshapes every root, so it
/// is a full-page navigation, and the choice is not persisted.
fn view_toggle(mode: ViewMode) -> Markup {
    html! {
        div.segmented role="group" aria-label="View" {
            @match mode {
                ViewMode::GapsOnly => {
                    span.segment.segment-active aria-current="page" { "Gaps only" }
                    a.segment href="/?view=all" { "All folders" }
                }
                ViewMode::All => {
                    a.segment href="/" { "Gaps only" }
                    span.segment.segment-active aria-current="page" { "All folders" }
                }
            }
        }
    }
}

async fn htmx_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript;charset=utf-8")],
        HTMX_JS,
    )
}

async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css;charset=utf-8")], APP_CSS)
}

/// The light/dark toggle button for the navbar. Behavior lives in `THEME_INIT_JS`.
fn theme_toggle() -> Markup {
    html! {
        button.btn.btn-ghost.btn-square.theme-toggle type="button"
            aria-label="Toggle light and dark theme"
            title="Toggle theme"
            onclick="toggleTheme()" { (PreEscaped(TOGGLE_SVG)) }
    }
}

/// The rotating folder caret used on collapsible rows.
fn chevron() -> Markup {
    html! { (PreEscaped(CHEVRON_SVG)) }
}

/// The folder glyph used on every node row (every node is a folder).
fn folder_icon() -> Markup {
    html! { (PreEscaped(FOLDER_SVG)) }
}

fn page(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Missing Ebooks" }
                link rel="icon" href=(FAVICON_HREF);
                script { (PreEscaped(THEME_INIT_JS)) }
                link rel="stylesheet" href="/static/app.css";
            }
            body {
                nav.navbar {
                    h1 { "Missing Ebooks" }
                    span.spacer {}
                    (view_toggle(mode))
                    (theme_toggle())
                    form method="post" action="/rescan" {
                        input type="hidden" name="view" value=(mode.as_query());
                        button.btn.btn-primary type="submit" { "Rescan" }
                    }
                }
                @for (root, section) in view.iter().enumerate() {
                    (render_section(section, root, None, links, mode))
                }
                script src="/static/htmx.min.js" {}
            }
        }
    }
}

/// Count the gaps (`needs_ebook()` nodes) anywhere in a forest. Drives the root
/// summary badge so a collapsed root still tells you how much is unresolved.
fn count_gaps(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| usize::from(n.needs_ebook()) + count_gaps(&n.children))
        .sum()
}

/// The badge shown on a root's summary: the gap count, a clean check, or a scan
/// error. In show-all the forest also holds covered nodes; only gaps are counted.
fn root_badge(state: &RootState) -> Markup {
    html! {
        @match state {
            RootState::Forest(nodes) => {
                @let n = count_gaps(nodes);
                @if n == 0 {
                    span.root-badge.root-badge-clean { "\u{2713} no gaps" }
                } @else if n == 1 {
                    span.root-badge.root-badge-gaps { "1 gap" }
                } @else {
                    span.root-badge.root-badge-gaps { (n) " gaps" }
                }
            }
            RootState::Clean => {
                span.root-badge.root-badge-clean { "\u{2713} no gaps" }
            }
            RootState::Error(_) => {
                span.root-badge.root-badge-error { "scan error" }
            }
        }
    }
}

fn render_section(
    section: &RootSection,
    root: usize,
    error: Option<&str>,
    links: &[SearchLink],
    mode: ViewMode,
) -> Markup {
    let counter = std::cell::Cell::new(0usize);
    html! {
        section.card.root {
            details.root-fold open {
                summary.root-head {
                    (chevron())
                    h2 { (section.path) }
                    span.spring {}
                    (root_badge(&section.state))
                }
                @if let Some(message) = error {
                    div.alert.alert-error { (PreEscaped(ERROR_SVG)) span { (message) } }
                }
                @match &section.state {
                    RootState::Forest(nodes) => {
                        @if nodes.is_empty() {
                            // Show-all yields an empty forest only for a root with no
                            // folders at all. Gaps-only sets Clean instead.
                            div.empty { span { "Nothing here" } }
                        } @else {
                            ul.menu {
                                @for node in nodes { (render_node(node, root, links, mode, &counter)) }
                            }
                        }
                    }
                    RootState::Clean => {
                        div.empty { (PreEscaped(CHECK_SVG)) span { "No missing ebooks in this root" } }
                    }
                    RootState::Error(message) => {
                        div.alert.alert-error {
                            (PreEscaped(ERROR_SVG)) span { "Could not scan this root: " (message) }
                        }
                    }
                }
            }
        }
    }
}

/// The show-all status marker for a row: a success check on a covered folder. Gaps
/// are already flagged by the amber icon and the badge, and plain containers need
/// no marker, so neither gets one. Rendered only in show-all mode.
fn status_icon(node: &Node) -> Markup {
    html! {
        @if !node.missing_ebook {
            span.status title="covered" { (PreEscaped(CHECK_SVG)) }
        }
    }
}

/// The covering ebook and marker filenames for a covered row, in muted text just
/// after the status check. Show-all only; empty for gaps and for folders covered
/// from above, so nothing renders there.
fn cover_files_span(node: &Node, mode: ViewMode) -> Markup {
    html! {
        @if mode == ViewMode::All && !node.cover_files.is_empty() {
            span.cover-files title="covering files" { (node.cover_files.join(", ")) }
        }
    }
}

fn render_node(
    node: &Node,
    root: usize,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    // A covered row dims only in show-all; gaps-only never holds covered nodes.
    let covered = mode == ViewMode::All && !node.missing_ebook;
    // Buttons and links appear only where there is a gap to act on. In gaps-only
    // every node qualifies, so the output is unchanged.
    let act = node.has_gap_within();
    html! {
        @if node.children.is_empty() {
            li {
                div.row.flagged[node.needs_ebook()].covered[covered] {
                    span.leaf-pad {}
                    (folder_icon())
                    span.name { (node.name) }
                    @if node.needs_ebook() { span.badge.badge-warning title="needs ebook" { "needs ebook" } }
                    @if mode == ViewMode::All { (status_icon(node)) }
                    (cover_files_span(node, mode))
                    span.spring {}
                    @if act {
                        (row_actions(root, &node.rel_path, &node.name, links, mode, counter))
                    }
                }
            }
        } @else {
            li {
                details open {
                    summary.row.flagged[node.needs_ebook()].covered[covered] {
                        (chevron())
                        (folder_icon())
                        span.name { (node.name) }
                        @if node.needs_ebook() { span.badge.badge-warning title="needs ebook" { "needs ebook" } }
                        @if mode == ViewMode::All { (status_icon(node)) }
                        (cover_files_span(node, mode))
                        span.spring {}
                        @if act {
                            (row_actions(root, &node.rel_path, &node.name, links, mode, counter))
                        }
                    }
                    ul {
                        @for child in &node.children { (render_node(child, root, links, mode, counter)) }
                    }
                }
            }
        }
    }
}

/// The per-row action cluster: a kebab trigger plus the marker buttons and the
/// search links, wrapped in a group that doubles as a native popover. On desktop
/// the trigger is hidden and the group is `display: contents`, so its children
/// flow inline in the row as before; on mobile the kebab opens the group as a
/// bottom action sheet over a dimmed backdrop. The browser provides the toggle,
/// one-open-at-a-time, light-dismiss, and Esc.
fn row_actions(
    root: usize,
    rel: &str,
    name: &str,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    let group_id = next_id("acts", root, counter);
    html! {
        button.actions-trigger type="button"
            aria-label="Actions"
            aria-haspopup="menu"
            popovertarget=(group_id)
            onclick="event.stopPropagation()" { (PreEscaped(KEBAB_SVG)) }
        div.actions-group id=(group_id) popover="auto" aria-label=(name) {
            div.sheet-header {
                span.sheet-grip aria-hidden="true" {}
                span.sheet-title { (name) }
            }
            (marker_buttons(root, rel, mode))
            (search_links(links, name, root, counter))
        }
    }
}

fn marker_buttons(root: usize, rel: &str, mode: ViewMode) -> Markup {
    html! {
        form.mark.actions hx-target="closest section.root" hx-swap="outerHTML" {
            input type="hidden" name="root" value=(root);
            input type="hidden" name="rel" value=(rel);
            input type="hidden" name="view" value=(mode.as_query());
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"no_ebook"}"#)
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(NO_ENTRY_SVG)) }
                    span.label-long { "Mark as None" }
                    span.label-short { "None" }
                }
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"ebook_elsewhere"}"#)
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(EBOOK_ELSEWHERE_SVG)) }
                    span.label-long { "Ebook elsewhere" }
                    span.label-short { "Elsewhere" }
                }
        }
    }
}

/// Next DOM-safe id with the given prefix for a row's popover or action group.
/// The relative path can hold slashes and spaces, so a root index plus a
/// per-render counter is used instead. The counter is reset per
/// `render_section`, so ids are unique within one render.
fn next_id(prefix: &str, root: usize, counter: &std::cell::Cell<usize>) -> String {
    let n = counter.get();
    counter.set(n + 1);
    format!("{prefix}-{root}-{n}")
}

fn search_links(
    links: &[SearchLink],
    name: &str,
    root: usize,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    html! {
        @if !links.is_empty() {
            @let query = urlencoding::encode(&clean_query(name)).into_owned();
            @let id = next_id("links", root, counter);
            span.links {
                span.sheet-divider { "Search" }
                button.btn.btn-outline.btn-xs.links-toggle type="button"
                    popovertarget=(id)
                    aria-label="Search for this book"
                    title="Search links"
                    onclick="event.stopPropagation()" { (PreEscaped(SEARCH_SVG)) }
                div.links-menu popover="auto" id=(id) onclick="event.stopPropagation()" {
                    @for link in links {
                        a href=(link.url.replace("{query}", &query))
                            target="_blank" rel="noopener noreferrer" {
                                span.sheet-icon { (PreEscaped(SEARCH_SVG)) }
                                (link.label)
                            }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::config::Config;
    use crate::scanner::ScanSettings;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    fn app_for(root: &Path) -> Router {
        app_for_with_links(root, Config::default().search_links)
    }

    fn app_for_with_links(root: &Path, search_links: Vec<SearchLink>) -> Router {
        let cfg = Config {
            library_roots: vec![root.to_path_buf()],
            ttl_seconds: 60,
            search_links,
            ..Default::default()
        };
        let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
        router(Arc::new(AppState::new(cfg, settings)))
    }

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn index_renders_a_flagged_folder() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("Book"));
    }

    #[tokio::test]
    async fn index_links_an_inline_favicon() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // An inline SVG data-URI favicon, so the browser stops requesting
        // /favicon.ico and the tab gets an identity.
        assert!(body.contains(r#"rel="icon""#));
        assert!(body.contains("data:image/svg+xml,"));
    }

    #[tokio::test]
    async fn index_shows_the_clean_message_for_a_covered_root() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        touch(&dir.path().join("Book/Book.epub"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("No missing ebooks in this root"));
    }

    #[tokio::test]
    async fn index_renders_the_marker_buttons_and_script() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains(r#"hx-post="/mark""#));
        assert!(body.contains(r#"src="/static/htmx.min.js""#));
        assert!(body.contains(">None<"));
    }

    #[tokio::test]
    async fn elsewhere_button_uses_the_book_check_icon() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The "Ebook elsewhere" button now carries a book-and-check glyph (the
        // checkmark path), not the old open-external-link arrow.
        assert!(body.contains("m9 9.5 2 2 4-4"));
        assert!(!body.contains("M10 14L21 3"));
    }

    #[tokio::test]
    async fn index_renders_the_search_links() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book (Unabridged)/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // Goodreads ships as a default link. The (Unabridged) suffix is stripped from
        // the query, so the href ends in `q=Book`, and the links open in a new tab.
        assert!(body.contains(r#"target="_blank""#));
        assert!(body.contains("https://www.goodreads.com/search?q=Book"));
        assert!(body.contains("Goodreads"));
    }

    #[tokio::test]
    async fn index_renders_every_configured_link() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        // The defaults ship two links; both must render, not just the first.
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("https://www.goodreads.com/search?q=Book"));
        assert!(body.contains("https://oceanofpdf.com/?s=Book"));
        assert!(body.contains("OceanofPDF"));
    }

    #[tokio::test]
    async fn index_omits_the_links_span_when_none_are_configured() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for_with_links(dir.path(), Vec::new())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // No links means no `span.links` is emitted, and no search popover menu.
        // The kebab still carries `popovertarget`; it is the sheet trigger now.
        assert!(!body.contains(r#"class="links""#));
        assert!(!body.contains(r#"class="links-menu""#));
        assert!(!body.contains(r#"title="Search links""#));
    }

    #[tokio::test]
    async fn search_links_render_inside_a_popover_menu() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A magnifying-glass button opens a popover that holds the links.
        assert!(body.contains("popovertarget"));
        assert!(body.contains(r#"class="links-menu""#));
        // The link itself is unchanged, just relocated into the menu.
        assert!(body.contains("https://www.goodreads.com/search?q=Book"));
        assert!(body.contains(r#"target="_blank""#));
    }

    #[tokio::test]
    async fn search_link_query_percent_encodes_spaces() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author Name/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // Spaces in the cleaned query are percent-encoded, so the href carries `%20`.
        assert!(body.contains("q=Author%20Name"));
    }

    #[tokio::test]
    async fn index_renders_the_menu_with_a_flagged_badge() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The tree is now a `menu`, and the styled section keeps the `root` hook.
        assert!(body.contains(r#"class="menu""#));
        assert!(body.contains(r#"class="card root""#));
        // A flagged folder carries the warning badge.
        assert!(body.contains("needs ebook"));
    }

    #[tokio::test]
    async fn index_links_the_stylesheet_and_inits_the_theme() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The external stylesheet replaces the old inline <style> block.
        assert!(body.contains(r#"href="/static/app.css""#));
        // The pre-paint theme script is present and reads the OS preference.
        assert!(body.contains("prefers-color-scheme"));
        // The toggle is labelled for assistive tech.
        assert!(body.contains(r#"aria-label="Toggle light and dark theme""#));
    }

    #[tokio::test]
    async fn static_route_serves_the_stylesheet() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers().get("content-type").unwrap();
        assert!(content_type.to_str().unwrap().contains("text/css"));
        let body = body_string(response).await;
        assert!(body.contains("--color-base-100"));
    }

    #[tokio::test]
    async fn stylesheet_carries_the_mobile_layout_rules() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("@media (max-width: 600px)"));
        assert!(body.contains(".actions-trigger"));
        // The action group is now a bottom sheet driven by the popover API, not
        // the old `data-actions-open` toggle.
        assert!(body.contains(":popover-open"));
        assert!(body.contains("::backdrop"));
        assert!(!body.contains("data-actions-open"));
    }

    #[tokio::test]
    async fn stylesheet_lays_out_marker_tiles_side_by_side() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The two marker buttons share a row as equal-width tiles.
        assert!(body.contains(".actions-group .mark .btn"));
        assert!(body.contains("flex-direction: row"));
    }

    #[tokio::test]
    async fn stylesheet_left_aligns_the_sheet_search_links() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The links column stretches to full width instead of centering, so the
        // search links share the marker buttons' left edge.
        assert!(body.contains("align-items: stretch"));
    }

    #[tokio::test]
    async fn stylesheet_collapses_the_flagged_badge_and_keeps_rows_on_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // On mobile the "needs ebook" pill collapses to an amber dot. The label is
        // pushed out of the box with the image-replacement idiom (text-indent), not
        // removed, so it stays in the HTML for screen readers.
        assert!(body.contains("text-indent: 100%"));
        // Non-covered rows stop wrapping, so the dot and kebab stay on the first
        // line and a long name wraps inside its own box instead.
        assert!(body.contains(".row:not(.covered)"));
        assert!(body.contains("overflow-wrap: anywhere"));
    }

    #[tokio::test]
    async fn stylesheet_stacks_the_navbar_view_toggle_into_a_full_width_row() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The segmented view toggle drops to its own row at full width, with the
        // two segments sharing it as equal-width halves.
        assert!(body.contains(".navbar .segmented"));
        assert!(body.contains("flex-basis: 100%"));
        assert!(body.contains(".navbar .segmented .segment"));
        // The theme toggle is ordered by a dedicated class, not a `> button`
        // child selector, so a later navbar button can't drift into its row.
        assert!(body.contains(".navbar .theme-toggle"));
        assert!(!body.contains(".navbar > button"));
    }

    #[tokio::test]
    async fn stylesheet_indents_the_mobile_cover_files_past_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // Covering filenames drop below the folder name and indent past where the
        // name starts, so they read as subordinate rather than lining up flush.
        assert!(body.contains(".cover-files"));
        assert!(body.contains("padding-left: 3.5rem"));
    }

    #[tokio::test]
    async fn the_flagged_badge_carries_a_hover_title() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The mobile dot has no visible text, so the badge gets a title that names
        // it on hover. The literal label is still emitted as the badge's content.
        assert!(body.contains(r#"title="needs ebook""#));
        assert!(body.contains("needs ebook"));
    }

    #[tokio::test]
    async fn static_route_serves_the_htmx_script() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/htmx.min.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers().get("content-type").unwrap();
        assert!(content_type.to_str().unwrap().contains("javascript"));
    }

    #[tokio::test]
    async fn rescan_redirects_to_root() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rescan")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/");
    }

    #[tokio::test]
    async fn mark_writes_the_file_and_swaps_the_section() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());
        // Warm the cache so the mark exercises the in-place update.
        app.clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("HX-Request", "true")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("No missing ebooks in this root"));
        assert!(dir.path().join("Book/.no_ebook").exists());
    }

    #[tokio::test]
    async fn all_view_renders_covered_folders_that_gaps_only_drops() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Gap/01.mp3"));
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let app = app_for(dir.path());

        // Gaps-only drops the covered book.
        let gaps = body_string(
            app.clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(!gaps.contains("Covered"));

        // Show-all renders it.
        let all = body_string(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(all.contains("Covered"));
        assert!(all.contains("Gap"));
    }

    #[tokio::test]
    async fn mark_in_all_mode_keeps_the_covered_folder_visible() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let app = app_for(dir.path());
        // Warm the all slot.
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/?view=all")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Author/Book&kind=no_ebook&view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        // The book stays in the tree (covered), not removed.
        assert!(body.contains("Book"));
        assert!(dir.path().join("Author/Book/.no_ebook").exists());
    }

    #[tokio::test]
    async fn rescan_preserves_the_current_view() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rescan")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/?view=all");
    }

    #[tokio::test]
    async fn mark_outside_a_root_shows_an_inline_error() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());
        app.clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=..&kind=no_ebook"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("Could not mark"));
        // The failed write leaves the tree intact.
        assert!(body.contains("Book"));
    }

    #[tokio::test]
    async fn all_view_dims_covered_rows_and_omits_their_buttons() {
        let dir = tempfile::tempdir().unwrap();
        // A covered container (series epub) whose books are all covered.
        touch(&dir.path().join("Series/Series.epub"));
        touch(&dir.path().join("Series/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // Covered rows carry the success check and the covered class.
        assert!(body.contains(r#"title="covered""#));
        assert!(body.contains(r#"covered""#));
        // A fully covered branch carries no marker buttons.
        assert!(!body.contains(r#"hx-post="/mark""#));
    }

    #[tokio::test]
    async fn all_view_keeps_buttons_on_a_container_above_a_gap() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Gap/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The author is a plain container above a gap, so it still gets buttons.
        assert!(body.contains(r#"hx-post="/mark""#));
        assert!(body.contains("Gap"));
    }

    #[tokio::test]
    async fn the_view_control_marks_the_active_segment() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let app = app_for(dir.path());

        let gaps = body_string(
            app.clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // Gaps-only is the active view; "All folders" is the link to the other view.
        assert!(gaps.contains(r#"class="segmented""#));
        assert!(gaps.contains("Gaps only"));
        assert!(gaps.contains("All folders"));
        assert!(gaps.contains(r#"href="/?view=all""#));

        let all = body_string(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // Show-all is active; "Gaps only" links back to /.
        assert!(all.contains(r#"href="/""#));
        assert!(all.contains(r#"aria-current="page""#));
    }

    #[tokio::test]
    async fn each_root_renders_a_collapsible_summary_with_a_gap_count() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The root head is now a <summary> inside a collapsible <details>.
        assert!(body.contains(r#"class="root-fold""#));
        assert!(body.contains("root-head"));
        // One gap under this root, so the badge reads "1 gap".
        assert!(body.contains("1 gap"));
    }

    #[tokio::test]
    async fn a_clean_root_badge_reads_no_gaps() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        touch(&dir.path().join("Book/Book.epub"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("no gaps"));
    }

    #[tokio::test]
    async fn all_view_shows_nothing_here_for_a_root_with_no_folders() {
        let dir = tempfile::tempdir().unwrap();
        // An empty root: no subfolders, no audio.
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("Nothing here"));
    }

    #[tokio::test]
    async fn all_view_lists_the_covering_ebook_on_a_covered_row() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains(r#"class="cover-files""#));
        assert!(body.contains("Covered.epub"));
    }

    #[tokio::test]
    async fn gaps_only_view_lists_no_cover_files() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Gap/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(!body.contains(r#"class="cover-files""#));
    }

    #[tokio::test]
    async fn marking_in_all_mode_shows_the_written_marker_on_the_row() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let app = app_for(dir.path());
        // Warm the all slot.
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/?view=all")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Author/Book&kind=no_ebook&view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains(r#"class="cover-files""#));
        assert!(body.contains(".no_ebook"));
    }

    #[tokio::test]
    async fn gaps_only_view_has_no_status_icons_or_covered_rows() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // No status markers and no covered rows in the gaps-only output.
        assert!(!body.contains(r#"class="status""#));
        assert!(!body.contains(r#"covered""#));
        // The gap and its buttons are still there.
        assert!(body.contains("Book"));
        assert!(body.contains(r#"hx-post="/mark""#));
    }

    #[tokio::test]
    async fn each_actionable_row_has_an_actions_trigger() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // A labelled kebab that opens the per-row action sheet via the native
        // popover API, and the group that is that popover.
        assert!(body.contains(r#"class="actions-trigger""#));
        assert!(body.contains(r#"aria-label="Actions""#));
        assert!(body.contains(r#"aria-haspopup="menu""#));
        assert!(body.contains("popovertarget"));
        assert!(body.contains(r#"class="actions-group""#));
        assert!(body.contains(r#"popover="auto""#));
        // The group is labelled with the folder name and titles the sheet with it.
        assert!(body.contains(r#"aria-label="Book""#));
        assert!(body.contains(r#"class="sheet-title""#));
        // The marker buttons and search links still render inside the group.
        assert!(body.contains(r#"hx-post="/mark""#));
        assert!(body.contains(">None<"));
        assert!(body.contains("Goodreads"));
        // The bespoke toggle is gone; the browser drives open/close now.
        assert!(!body.contains("toggleRowActions"));
        assert!(!body.contains("aria-expanded"));
    }

    #[tokio::test]
    async fn the_action_sheet_titles_with_the_folder_and_shows_verbose_labels() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The sheet header titles the sheet with the folder name.
        assert!(body.contains(r#"class="sheet-title">Book<"#));
        // The verbose, sheet-only marker labels sit alongside the compact ones.
        assert!(body.contains("Mark as None"));
        assert!(body.contains("Ebook elsewhere"));
        // The compact labels keep the exact text the marker write asserts on.
        assert!(body.contains(">None<"));
        assert!(body.contains(">Elsewhere<"));
    }

    #[tokio::test]
    async fn the_action_sheet_marks_the_search_section() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A sheet-only "Search" divider separates the marker rows from the links.
        assert!(body.contains(r#"class="sheet-divider""#));
        // The links still resolve to their configured search URLs.
        assert!(body.contains("https://www.goodreads.com/search?q=Book"));
    }

    #[tokio::test]
    async fn a_covered_row_has_no_actions_trigger() {
        let dir = tempfile::tempdir().unwrap();
        // A fully covered branch: the book has its own ebook, nothing to act on.
        touch(&dir.path().join("Series/Series.epub"));
        touch(&dir.path().join("Series/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // No gap under this branch, so no trigger and no group are emitted.
        assert!(!body.contains(r#"class="actions-trigger""#));
        assert!(!body.contains(r#"class="actions-group""#));
    }
}
