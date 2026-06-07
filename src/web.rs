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

/// The vendored htmx runtime, embedded at compile time and served from /static.
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

/// The hand-rolled stylesheet, embedded at compile time and served from /static.
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

/// Folder glyph shown on every node row. Inherits `currentColor`.
const FOLDER_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"##;

/// Check mark for the "no gaps in this root" state. Inherits `currentColor`.
const CHECK_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M5 13l4 4L19 7"/></svg>"##;

/// Circled exclamation for a scan or write error. Inherits `currentColor`.
const ERROR_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M12 8v4M12 16h.01"/></svg>"##;

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

/// The gaps-only / show-all toggle for the navbar: a plain GET link styled as a
/// button. Switching modes reshapes every root, so this is a full-page navigation.
fn view_toggle(mode: ViewMode) -> Markup {
    html! {
        @match mode {
            ViewMode::GapsOnly => a.btn.btn-ghost href="/?view=all" { "Show all folders" },
            ViewMode::All => a.btn.btn-ghost href="/" { "Show gaps only" },
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

/// The light/dark toggle button for the navbar. Behavior lives in THEME_INIT_JS.
fn theme_toggle() -> Markup {
    html! {
        button.btn.btn-ghost.btn-square type="button"
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

fn render_section(
    section: &RootSection,
    root: usize,
    error: Option<&str>,
    links: &[SearchLink],
    mode: ViewMode,
) -> Markup {
    html! {
        section.card.root {
            div.root-head { h2 { (section.path) } }
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
                            @for node in nodes { (render_node(node, root, links, mode)) }
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

fn render_node(node: &Node, root: usize, links: &[SearchLink], mode: ViewMode) -> Markup {
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
                    @if node.needs_ebook() { span.badge.badge-warning { "needs ebook" } }
                    @if mode == ViewMode::All { (status_icon(node)) }
                    span.spring {}
                    @if act {
                        (marker_buttons(root, &node.rel_path, mode))
                        (search_links(links, &node.name))
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
                        @if node.needs_ebook() { span.badge.badge-warning { "needs ebook" } }
                        @if mode == ViewMode::All { (status_icon(node)) }
                        span.spring {}
                        @if act {
                            (marker_buttons(root, &node.rel_path, mode))
                            (search_links(links, &node.name))
                        }
                    }
                    ul {
                        @for child in &node.children { (render_node(child, root, links, mode)) }
                    }
                }
            }
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
                onclick="event.stopPropagation()" { "No ebook" }
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"ebook_elsewhere"}"#)
                onclick="event.stopPropagation()" { "Elsewhere" }
        }
    }
}

fn search_links(links: &[SearchLink], name: &str) -> Markup {
    html! {
        @if !links.is_empty() {
            @let query = urlencoding::encode(&clean_query(name)).into_owned();
            span.links {
                @for (i, link) in links.iter().enumerate() {
                    @if i > 0 { span.sep { "·" } }
                    a href=(link.url.replace("{query}", &query))
                        target="_blank" rel="noopener noreferrer" { (link.label) }
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
        assert!(body.contains("No ebook"));
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
        // No links means no `span.links` is emitted. The CSS rule in <style> names
        // `span.links`, so match the rendered attribute, which appears only on a row.
        assert!(!body.contains(r#"class="links""#));
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
    async fn the_toggle_points_at_the_other_mode() {
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
        assert!(all.contains(r#"href="/""#));
        assert!(all.contains("Show gaps only"));
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
}
