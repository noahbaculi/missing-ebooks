//! axum router, request handlers, and Maud markup. Handlers are thin: they call a
//! `service` operation and render. Handlers return `Html<String>` so Maud stays
//! decoupled from the axum version. Marker writes use htmx to swap just the
//! affected root's section; the script is vendored and served from `/static`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use maud::Markup;
use serde::Deserialize;

use crate::config::SearchLink;
use crate::marker::Marker;
use crate::service::{self, ViewMode};
use crate::state::AppState;

mod assets;
mod render;

// The demo router reuses these two handlers; re-export so its `use crate::web::…`
// import path stays put.
pub(crate) use assets::{app_css, htmx_script};

// The demo router reuses the page and section renderers; re-export so its
// `use crate::web::…` import path stays put.
pub(crate) use render::{page, render_section};

/// The `view` parameter shared by the index query string and the rescan form. A
/// lenient `Option<String>` so an absent or unknown value falls back to gaps-only
/// rather than rejecting the request.
#[derive(Deserialize)]
pub(crate) struct ViewQuery {
    #[serde(default)]
    pub(crate) view: Option<String>,
}

/// The body of a marker write: which root, which folder, which marker, and which
/// view the click came from (so the swapped section comes back in that mode).
#[derive(Deserialize)]
pub(crate) struct MarkRequest {
    pub(crate) root: usize,
    pub(crate) rel: String,
    pub(crate) kind: Marker,
    #[serde(default)]
    pub(crate) view: ViewMode,
}

/// Build the application router with the shared state attached.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/mark", post(mark))
        .route("/unmark", post(unmark))
        .route("/rescan", post(rescan))
        .route("/static/htmx.min.js", get(assets::htmx_script))
        .route("/static/app.css", get(assets::app_css))
        .route("/static/app.js", get(assets::app_js))
        .with_state(state)
}

async fn index(State(state): State<Arc<AppState>>, Query(query): Query<ViewQuery>) -> Html<String> {
    let mode = ViewMode::from_query(query.view.as_deref());
    let view = service::current_view(&state, mode).await;
    Html(render::page(&view, &state.config.search_links, mode).into_string())
}

async fn mark(
    State(state): State<Arc<AppState>>,
    Form(req): Form<MarkRequest>,
) -> axum::response::Response {
    let links = &state.config.search_links;
    let mode = req.view;
    match service::mark(&state, req.root, &req.rel, req.kind, mode).await {
        Ok(outcome) => {
            let markup =
                render::render_section(&outcome.view[req.root], req.root, None, links, mode);
            let trigger = outcome.created.then(|| {
                let name = display_name(&outcome.view[req.root].path, &req.rel);
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
    }
}

async fn unmark(
    State(state): State<Arc<AppState>>,
    Form(req): Form<MarkRequest>,
) -> axum::response::Response {
    let links = &state.config.search_links;
    let mode = req.view;
    match service::unmark(&state, req.root, &req.rel, req.kind, mode).await {
        Ok(view) => section_response(
            render::render_section(&view[req.root], req.root, None, links, mode),
            None,
        ),
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
    }
}

/// A server-side write failed: re-render the affected root's section with an
/// inline alert that names the folder, so the error stays on the page by the row
/// rather than in a toast. The tree is left intact and the row keeps its buttons.
/// The current view is re-fetched (a cache hit) since the failed call returned no
/// view; an out-of-range root falls back to a standalone error card.
async fn failed_write_response(
    state: &AppState,
    root: usize,
    mode: ViewMode,
    links: &[SearchLink],
    message: String,
) -> axum::response::Response {
    let view = service::current_view(state, mode).await;
    let markup = match view.get(root) {
        Some(section) => render::render_section(section, root, Some(&message), links, mode),
        None => render::error_section(root, &message),
    };
    section_response(markup, None)
}

async fn rescan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(query): Form<ViewQuery>,
) -> Response {
    let mode = ViewMode::from_query(query.view.as_deref());
    let view = service::rescan(&state, mode).await;
    if headers.contains_key("HX-Request") {
        // htmx path: swap the fresh sections into #roots, and push the mode path so
        // the address bar tracks the view without ever showing the /rescan POST URL.
        let markup = render::roots(&view, &state.config.search_links, mode);
        (
            [("HX-Push-Url", mode_path(mode))],
            Html(markup.into_string()),
        )
            .into_response()
    } else {
        // no-JS path: 303 See Other (Post/Redirect/Get), so a refresh does not
        // re-trigger a scan.
        Redirect::to(mode_path(mode)).into_response()
    }
}

/// The path that renders a given mode, for redirects and links.
fn mode_path(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::GapsOnly => "/",
        ViewMode::All => "/?view=all",
    }
}

/// JSON-escape any non-ASCII char to `\uXXXX`, so an `HX-Trigger` header value is
/// pure ASCII and survives any browser header decoding. The input is valid JSON;
/// replacing a raw char with its escape keeps it valid JSON. Folder names are
/// often non-ASCII, so this is the common path, not an edge case.
fn ascii_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u16; 2];
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// Render a section response, optionally carrying an `HX-Trigger` header. A value
/// that will not encode (a control char in a folder name) is dropped rather than
/// failing the response: the swap still happens, only the toast is skipped.
fn section_response(markup: Markup, trigger: Option<String>) -> axum::response::Response {
    let mut resp = Html(markup.into_string()).into_response();
    if let Some(value) = trigger
        && let Ok(header) = HeaderValue::from_str(&value)
    {
        resp.headers_mut()
            .insert(HeaderName::from_static("hx-trigger"), header);
    }
    resp
}

/// The folder's display name for the toast: the last path segment, or the root
/// label (the section path's last component) when the target is the root itself.
fn display_name(section_path: &str, rel: &str) -> String {
    if rel == "." {
        std::path::Path::new(section_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(section_path)
            .to_string()
    } else {
        rel.rsplit('/').next().unwrap_or(rel).to_string()
    }
}

/// The `HX-Trigger` payload for a successful create: a `marked` event carrying
/// what the toast needs to describe and to undo the write.
fn marked_trigger(req: &MarkRequest, name: &str) -> String {
    let payload = serde_json::json!({
        "marked": {
            "root": req.root,
            "rel": req.rel,
            "kind": req.kind,
            "view": req.view.as_query(),
            "name": name,
        }
    });
    ascii_escape(&payload.to_string())
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

    use crate::config::{Config, SearchLink};
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

    fn app_for_roots(roots: &[&Path]) -> Router {
        let cfg = Config {
            library_roots: roots.iter().map(|r| r.to_path_buf()).collect(),
            ttl_seconds: 60,
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
    async fn index_tags_container_rows_by_depth() {
        let dir = tempfile::tempdir().unwrap();
        // Author (top container) -> Series (nested container) -> Book (flagged leaf).
        touch(&dir.path().join("Author/Series/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The top container is tagged for bold, the nested one for italic.
        assert!(body.contains(r#"class="row container-top""#));
        assert!(body.contains(r#"class="row container-nested""#));
        // The flagged leaf keeps exactly its existing class, with no depth tag.
        assert!(body.contains(r#"class="row flagged""#));
    }

    #[tokio::test]
    async fn index_marks_loose_and_mixed_flagged_folders() {
        let dir = tempfile::tempdir().unwrap();
        // A loose gap: a book folder at the very top, with no author folder around it.
        touch(&dir.path().join("The Hobbit/01.mp3"));
        // A mixed node: an author folder that holds a loose file and also a gap subfolder.
        touch(&dir.path().join("Terry Pratchett/01.mp3"));
        touch(&dir.path().join("Terry Pratchett/Going Postal/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("loose at top"), "the top-level book is marked loose");
        assert!(
            body.contains("holds audio + subfolders"),
            "the half-sorted author is marked mixed"
        );
    }

    #[tokio::test]
    async fn index_shows_a_file_count_and_a_collapsed_file_list_on_a_flagged_leaf() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01 - The Gunslinger.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The count sits on the row, and the file row is present but inside a closed
        // <details> (no `open`), so the names are hidden until the row is expanded.
        assert!(body.contains("1 file"));
        assert!(body.contains(r#"<details class="node-files">"#));
        assert!(body.contains("01 - The Gunslinger.mp3"));
    }

    #[tokio::test]
    async fn index_pluralizes_the_file_count() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        touch(&dir.path().join("Book/02.mp3"));
        touch(&dir.path().join("Book/03.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("3 files"));
    }

    #[tokio::test]
    async fn mixed_node_shows_its_own_files_above_its_child_gap() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Terry Pratchett/01 - The Colour of Magic.mp3"));
        touch(&dir.path().join("Terry Pratchett/Going Postal/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The mixed author's own loose file renders as a file row, and the child gap
        // still renders as a folder row carrying its badge.
        assert!(body.contains("01 - The Colour of Magic.mp3"));
        assert!(body.contains(r#"class="file-row""#));
        assert!(body.contains("Going Postal"));
    }

    #[tokio::test]
    async fn index_leaves_a_deep_gap_unmarked() {
        let dir = tempfile::tempdir().unwrap();
        // A properly filed gap two levels down carries no smell.
        touch(&dir.path().join("Author/Series/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(!body.contains("loose at top"));
        assert!(!body.contains("holds audio + subfolders"));
    }

    #[tokio::test]
    async fn show_all_keeps_depth_tags_on_covered_containers() {
        let dir = tempfile::tempdir().unwrap();
        // Audio under a series, plus an ebook at the author level so the whole
        // branch is covered in show-all.
        touch(&dir.path().join("Author/Series/Book/01.mp3"));
        touch(&dir.path().join("Author/Author.epub"));
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
        // The covered top and nested containers still carry their depth tags, so the
        // depth cue survives the view switch and composes with the covered class.
        assert!(body.contains(r#"class="row covered container-top""#));
        assert!(body.contains(r#"class="row covered container-nested""#));
        // The covered leaf book carries the bare covered class with no depth tag
        // (the trailing quote rules out a `covered container-*` prefix match).
        assert!(body.contains(r#"class="row covered""#));
    }

    #[tokio::test]
    async fn index_wraps_the_sections_in_a_roots_container() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The root sections live inside a positioned wrapper so the rescan skeleton
        // can overlay them, and inside #roots so htmx can swap them in place.
        assert!(body.contains(r#"class="roots-wrap""#));
        assert!(body.contains(r#"id="roots""#));
        // The sections themselves are unchanged.
        assert!(body.contains(r#"class="card root""#));
    }

    #[tokio::test]
    async fn marker_form_delays_the_swap_only_in_gaps_only() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());
        // Gaps-only: the marked folder leaves the list, so the section swap is delayed
        // to let app.js play the row's collapse before the fresh section lands.
        let gaps = body_string(
            app.clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(gaps.contains(r#"hx-swap="outerHTML swap:250ms""#));
        // Show-all: the row flips to covered in place, so the swap is immediate; the
        // reserved row height keeps the flip from shifting the rows below.
        let all = body_string(
            app.oneshot(
                Request::builder()
                    .uri("/?view=all")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert!(all.contains(r#"hx-swap="outerHTML""#));
        assert!(!all.contains("swap:250ms"));
    }

    #[tokio::test]
    async fn app_script_collapses_the_leaving_row() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // Before a gaps-only mark request goes out, the script collapses the leaving
        // row so the rows below glide up; the delayed htmx swap reconciles after.
        assert!(body.contains("htmx:beforeRequest"));
        assert!(body.contains("leaving"));
        // collapseRow walks up from the marked leaf and collapses each ancestor that is
        // the sole `:scope > li` in its list, so an emptied author or series row leaves
        // with the leaf instead of snapping out on the swap.
        assert!(body.contains(":scope > li"));
    }

    #[tokio::test]
    async fn app_script_blurs_the_mark_button_before_the_swap() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The section swap removes the focused mark button. Left focused, the browser
        // jumps the scroll to the page bottom (true in both views), so the script
        // drops focus before the swap.
        assert!(body.contains("blur"));
    }

    #[tokio::test]
    async fn stylesheet_collapses_the_leaving_row_and_respects_reduced_motion() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // A leaving row collapses its height and fades; motion-sensitive users get the
        // instant removal instead.
        assert!(body.contains(".leaving"));
        assert!(body.contains("max-height"));
        assert!(body.contains("prefers-reduced-motion"));
    }

    #[tokio::test]
    async fn stylesheet_styles_container_depth() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The top container of a tree is bold; containers nested below it are italic.
        assert!(body.contains(".container-top .name"));
        assert!(body.contains(".container-nested .name"));
        assert!(body.contains("font-style: italic"));
    }

    #[tokio::test]
    async fn rescan_is_an_in_place_htmx_swap_with_a_skeleton() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // Rescan posts via htmx and swaps the fresh sections into #roots.
        assert!(body.contains(r#"hx-post="/rescan""#));
        assert!(body.contains(r##"hx-target="#roots""##));
        // The skeleton lights up as an indicator, and the button is disabled for the
        // request so a second click cannot fire a second scan.
        assert!(body.contains(r##"hx-indicator="#scan-skeleton, #rescan-btn""##));
        assert!(body.contains(r##"hx-disabled-elt="#rescan-btn""##));
        // The skeleton overlay is present and wired as the htmx indicator.
        assert!(body.contains(r#"id="scan-skeleton""#));
        assert!(body.contains("htmx-indicator"));
        // The button keeps its constant "Rescan" label and locks via hx-disabled-elt
        // (asserted above); it no longer relabels while the scan runs.
        assert!(body.contains(r#"id="rescan-btn""#));
        assert!(body.contains("Rescan"));
        assert!(!body.contains("Rescanning"));
        // The plain form action survives for the no-JS path.
        assert!(body.contains(r#"action="/rescan""#));
    }

    #[tokio::test]
    async fn rescan_returns_sections_for_an_htmx_request() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rescan")
                    .header("HX-Request", "true")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        // No redirect: the fresh sections come back to swap into #roots.
        assert_eq!(response.status(), StatusCode::OK);
        // The address bar is pushed to the requested view, not the POST URL.
        assert_eq!(response.headers().get("HX-Push-Url").unwrap(), "/?view=all");
        let body = body_string(response).await;
        assert!(body.contains(r#"class="card root""#));
        assert!(body.contains("Book"));
    }

    #[tokio::test]
    async fn stylesheet_carries_the_scan_skeleton_shimmer() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The rescan placeholder is a positioned overlay with a shimmer animation.
        assert!(body.contains(".scan-skeleton"));
        assert!(body.contains("@keyframes shimmer"));
        // The wrapper is positioned so the overlay can pin to it.
        assert!(body.contains(".roots-wrap"));
        // The rescan button dims and shows a locked cursor while disabled.
        assert!(body.contains("#rescan-btn:disabled"));
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
        // A backdrop-less audiobook glyph that recolors with the OS theme: indigo
        // on light tab strips, a lighter indigo on dark ones. Pin the indigo and
        // the prefers-color-scheme rule so a revert to the old book glyph or to a
        // single static color is caught, and assert the rounded tile is gone (its
        // rect was 22x22).
        assert!(body.contains("%23605dff"));
        assert!(body.contains("prefers-color-scheme:dark"));
        assert!(!body.contains("width='22'"));
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
        assert!(body.contains(r#"src="/static/app.js""#));
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
        // The theme toggle moved into the settings menu: a labelled cog, with the
        // theme choices inside the panel.
        assert!(body.contains(r#"aria-label="Settings""#));
        assert!(body.contains(r#"data-theme-choice="system""#));
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
        // two segments sharing it as equal-width halves. A child combinator scopes
        // the rule to the navbar's own control, so the settings panel's nested
        // theme segmented control can't inherit the full-width row layout.
        assert!(body.contains(".navbar > .segmented"));
        assert!(body.contains("flex-basis: 100%"));
        assert!(body.contains(".navbar > .segmented .segment"));
        // The settings cog is ordered by a dedicated class, not a `> button`
        // child selector, so a later navbar button can't drift into its row.
        assert!(body.contains(".navbar .settings-cog"));
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
    async fn static_route_serves_the_app_script() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
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
    async fn app_script_defines_the_theme_setter() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("setTheme"));
        assert!(body.contains("confirmMarks"));
    }

    #[tokio::test]
    async fn navbar_renders_a_settings_cog_with_theme_and_confirm_controls() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // A labelled cog opens the settings panel via the native popover API.
        assert!(body.contains(r#"class="btn btn-ghost btn-square settings-cog""#));
        assert!(body.contains(r#"aria-label="Settings""#));
        assert!(body.contains(r#"popovertarget="settings-panel""#));
        assert!(body.contains(r#"id="settings-panel""#));
        // The theme segmented control offers all three states.
        assert!(body.contains(r#"data-theme-choice="light""#));
        assert!(body.contains(r#"data-theme-choice="dark""#));
        assert!(body.contains(r#"data-theme-choice="system""#));
        // The confirm-before-marking switch.
        assert!(body.contains(r#"id="confirm-toggle""#));
        // The old two-state toggle is gone.
        assert!(!body.contains("toggleTheme()"));
        // Confirm-before-marking renders above the theme control.
        let confirm_at = body.find(r#"id="confirm-toggle""#).unwrap();
        let theme_at = body.find(r#"data-theme-choice="light""#).unwrap();
        assert!(
            confirm_at < theme_at,
            "the confirm row should render above the theme control"
        );
    }

    #[tokio::test]
    async fn index_renders_the_hidden_connection_banner() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A polite live region, hidden until the connection JS reveals it. The
        // `hidden` check is anchored to the banner's own attributes so it can't pass
        // on an unrelated `aria-hidden` elsewhere on the page.
        assert!(body.contains(r#"id="conn-banner""#));
        assert!(body.contains(r#"role="status""#));
        assert!(body.contains(r#"role="status" aria-live="polite" hidden"#));
        // Copy lives in data attributes so it is locked here and read by app.js.
        assert!(body.contains(r#"data-msg-offline="You're offline. Changes can't be saved.""#));
        assert!(body.contains(r#"data-msg-retrying="Lost connection. Retrying…""#));
        assert!(
            body.contains(
                r#"data-msg-failed="Couldn't reach the server. Your change wasn't saved.""#
            )
        );
        assert!(body.contains(
            r#"data-msg-failed-rescan="Couldn't reach the server. The library wasn't rescanned.""#
        ));
        assert!(body.contains(r#"data-msg-reconnected="Reconnected.""#));
        // The message slot the JS fills.
        assert!(body.contains(r#"class="conn-banner-msg""#));
    }

    #[tokio::test]
    async fn index_renders_the_confirm_dialog() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // A single page-level dialog the confirm flow fills and opens.
        assert!(body.contains(r#"id="confirm-mark""#));
        assert!(body.contains("Don't ask again"));
        assert!(body.contains(r#"id="confirm-accept""#));
        assert!(body.contains(r#"id="confirm-cancel""#));
    }

    #[tokio::test]
    async fn index_renders_the_toast_stack_and_template() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // An empty stack container plus a template the script clones per toast, so
        // up to three coexist and survive the htmx section swaps.
        assert!(body.contains(r#"id="toast-stack""#));
        assert!(body.contains(r#"id="toast-template""#));
        // The template toast carries the undo button and the message slot.
        assert!(body.contains("toast-undo"));
        assert!(body.contains(r#"class="toast-msg""#));
    }

    #[tokio::test]
    async fn marker_buttons_carry_confirm_metadata() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // Each marker button names its action, file, and folder for the dialog.
        assert!(body.contains(r#"data-confirm-action="Mark as None""#));
        assert!(body.contains(r#"data-confirm-file=".no_ebook""#));
        assert!(body.contains(r#"data-confirm-action="Ebook elsewhere""#));
        assert!(body.contains(r#"data-confirm-file=".ebook_elsewhere""#));
        assert!(body.contains(r#"data-confirm-folder="Book""#));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_confirm_dialog() {
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
        // The dialog is themed and dims the page behind it.
        assert!(body.contains(".confirm-dialog"));
        assert!(body.contains(".confirm-dialog::backdrop"));
        // The non-matching marker glyph hides via the `hidden` attribute. The
        // explicit `.confirm-icon` display must honor it, or both glyphs show.
        assert!(body.contains(".confirm-icon[hidden]"));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_toast_and_its_variants() {
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
        // The stack container and the toast box.
        assert!(body.contains(".toast-stack"));
        assert!(body.contains(".toast"));
        // The success toast reveals its glyph in a tinted status badge.
        assert!(body.contains(".toast--success .toast-icon-success"));
        // The badge glyph resets the muted `.icon` color so it takes the variant
        // color; without this the glyph renders grey.
        assert!(body.contains(".toast .toast-icon .icon"));
        // The two-line message: the folder name over the outcome and label pill.
        assert!(body.contains(".toast-name"));
        assert!(body.contains(".toast-detail"));
        assert!(body.contains(".toast-kind"));
        // The arrival animation.
        assert!(body.contains("@keyframes toast-in"));
        // The arrival: a soft `ease` curve over 380ms, sliding the 1.4rem distance.
        // Brisk enough to register beside the row collapse, still gentle.
        assert!(body.contains("toast-in 380ms ease"));
        assert!(body.contains("translateY(1.4rem)"));
        // The matching slower exit.
        assert!(body.contains("toast-out 480ms ease-in"));
        // A settled toast drops its filled entry animation so the script can
        // slide it to a new position when another toast pushes in.
        assert!(body.contains(".toast--settled"));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_settings_panel_and_switch() {
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
        // The settings popover, the switch, and the mobile bottom-sheet form.
        assert!(body.contains(".settings-panel"));
        assert!(body.contains(".switch-track"));
        assert!(body.contains(".settings-panel:popover-open"));
        // The panel opens as a centered overlay (so the cog and the ? hotkey land it
        // in the same place) and dims the page behind it with a backdrop scrim.
        assert!(body.contains(".settings-panel::backdrop"));
        // The shortcuts reference is styled inside the panel and hidden on mobile.
        assert!(body.contains(".settings-shortcuts"));
    }

    #[tokio::test]
    async fn stylesheet_neutralizes_native_button_chrome_on_segments() {
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
        // The theme control renders its segments as <button>, the view toggle as
        // <span>/<a>. Without an appearance reset the buttons inherit the user
        // agent's native control chrome (grey fills, beveled borders) while the
        // view toggle stays flat, so the two diverge. The .segment rule must drop
        // that chrome so both render identically.
        assert!(body.contains(".segment"));
        assert!(body.contains("appearance: none"));
    }

    #[tokio::test]
    async fn app_script_intercepts_marker_writes() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("htmx:confirm"));
    }

    #[tokio::test]
    async fn app_script_defines_the_toast_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The success listener and the undo POST to /unmark.
        assert!(body.contains(r#"addEventListener("marked""#));
        assert!(body.contains("/unmark"));
        // The script drives a stack container and clones a per-toast template.
        assert!(body.contains("toast-stack"));
        assert!(body.contains("toast-template"));
        // The exit-removal delay is a named constant kept in step with the CSS
        // `toast-out` duration.
        assert!(body.contains("EXIT_MS"));
        // The auto-dismiss pauses while the toast is hovered or keyboard-focused.
        assert!(body.contains(r#"addEventListener("mouseenter""#));
        assert!(body.contains(r#"addEventListener("focusin""#));
        // Adding a toast slides the existing ones to their new spot (FLIP, via
        // getBoundingClientRect) over a shared reflow duration rather than
        // letting them jump.
        assert!(body.contains("REFLOW_MS"));
        assert!(body.contains("getBoundingClientRect"));
    }

    #[tokio::test]
    async fn app_script_toggles_the_summary_end_state() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The recompute shows the all-clear line and hides the hero-and-bar head once
        // the live total reaches zero, and reverses it when an undo brings a gap back,
        // so the live strip lands on the same end-state a reload would render.
        assert!(body.contains(r#"getElementById("gap-summary-clear")"#));
        assert!(body.contains(r#"getElementById("gap-summary-head")"#));
        assert!(body.contains("clear.hidden = total !== 0"));
        assert!(body.contains("head.hidden = total === 0"));
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
    async fn mark_sets_the_marked_trigger_on_a_create_and_omits_it_on_a_remark() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());

        // First mark creates the file: the response carries the marked trigger.
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let trigger = first
            .headers()
            .get("hx-trigger")
            .map(|v| v.to_str().unwrap().to_string());
        let trigger = trigger.expect("a create sets HX-Trigger");
        assert!(trigger.contains("marked"));
        assert!(trigger.contains("\"name\":\"Book\""));
        assert!(trigger.contains("\"kind\":\"no_ebook\""));

        // Second mark of the same folder is a no-op create: no marked trigger.
        let second = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(second.headers().get("hx-trigger").is_none());
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
    async fn section_carries_a_data_root_hook() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The toast's Undo targets the section by index, so each section names it.
        assert!(body.contains(r#"data-root="0""#));
    }

    #[tokio::test]
    async fn mark_failure_renders_an_inline_alert_and_keeps_the_tree() {
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
                    .body(Body::from("root=0&rel=..&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        // A server-side failure stays on the page: no HX-Trigger toast, just an inline
        // alert that names the folder, with the tree left intact.
        assert!(response.headers().get("hx-trigger").is_none());
        let body = body_string(response).await;
        assert!(body.contains("Book"));
        assert!(body.contains("alert-error"));
        assert!(body.contains("Could not mark"));
    }

    #[tokio::test]
    async fn unmark_route_deletes_the_file_and_swaps_the_section_back() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());
        app.clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        // Mark first so there is a file to remove.
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(dir.path().join("Book/.no_ebook").exists());

        // Undo: the file is gone and the swapped section shows the gap again.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/unmark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(!dir.path().join("Book/.no_ebook").exists());
        assert!(body.contains("Book"));
        assert!(body.contains("needs ebook"));
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

    #[tokio::test]
    async fn static_assets_carry_cache_control_and_a_strong_etag() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(dir.path());
        for path in ["/static/htmx.min.js", "/static/app.css", "/static/app.js"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let cache_control = response
                .headers()
                .get("cache-control")
                .unwrap()
                .to_str()
                .unwrap();
            assert!(
                cache_control.contains("max-age="),
                "{path} cache-control: {cache_control}"
            );
            let etag = response.headers().get("etag").unwrap().to_str().unwrap();
            // A strong validator: quoted, with no weak `W/` prefix.
            assert!(
                etag.starts_with('"') && etag.ends_with('"'),
                "{path} etag: {etag}"
            );
            assert!(!etag.starts_with("W/"), "{path} etag: {etag}");
        }
    }

    #[tokio::test]
    async fn htmx_is_cached_for_a_finite_window_and_is_not_immutable() {
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
        let cache_control = response
            .headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap();
        // A long but finite window for the vendored runtime, never immutable, so a
        // version bump still revalidates once the window passes.
        assert!(cache_control.contains("max-age=604800"));
        assert!(!cache_control.contains("immutable"));
    }

    #[tokio::test]
    async fn a_matching_if_none_match_gets_a_304_with_no_body() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(dir.path());
        // First request reads the asset's ETag.
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let etag = first
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Sending it back as If-None-Match revalidates: 304, the headers, no body.
        let revalidated = app
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .header("if-none-match", &etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
        assert!(revalidated.headers().get("etag").is_some());
        assert!(revalidated.headers().get("cache-control").is_some());
        let body = body_string(revalidated).await;
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn a_non_matching_if_none_match_gets_the_full_200() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .header("if-none-match", "\"stale\"")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("--color-base-100"));
    }

    #[tokio::test]
    async fn a_star_if_none_match_gets_a_304() {
        let dir = tempfile::tempdir().unwrap();
        // `If-None-Match: *` means "if any current representation exists", and the
        // asset always exists, so the conditional GET revalidates to a 304.
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .header("if-none-match", "*")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        let body = body_string(response).await;
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn index_renders_the_gap_summary_strip() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The strip renders server-side, between the navbar and the roots.
        assert!(body.contains(r#"id="gap-summary""#));
        // The hero gap total has its own hook, and the session bar's load-time
        // baseline rides on the strip as a data attribute. One gap here (Book).
        assert!(body.contains(r#"id="gap-total""#));
        assert!(body.contains(r#"data-gaps-at-load="1""#));
        // The session coverage readout: resolved of baseline audiobooks with a
        // percent, under a label that names the root it spans. First paint is 0 of 1.
        assert!(body.contains(r#"id="gap-resolved""#));
        assert!(body.contains(r#"id="gap-baseline""#));
        assert!(body.contains(r#"id="gap-pct""#));
        assert!(body.contains("audiobooks"));
        assert!(body.contains("Coverage in"));
        // The all-clear line renders too, hidden until the live total reaches zero.
        assert!(body.contains(r#"id="gap-summary-clear" hidden"#));
    }

    #[tokio::test]
    async fn gap_summary_shows_all_clear_for_a_covered_library() {
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
        // Total zero: the all-clear line shows and the zero baseline rides the strip.
        // The hero-and-bar head still renders so an undo back from the last mark can
        // bring it back, but it loads hidden.
        assert!(body.contains(r#"data-gaps-at-load="0""#));
        assert!(body.contains("All clear"));
        assert!(body.contains(r#"id="gap-summary-clear">"#));
        assert!(body.contains(r#"id="gap-summary-head" hidden"#));
    }

    #[tokio::test]
    async fn gap_summary_labels_the_session_coverage_with_every_root() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        touch(&a.path().join("BookA/01.mp3"));
        touch(&b.path().join("BookB/01.mp3"));
        let a_name = a.path().file_name().unwrap().to_str().unwrap();
        let b_name = b.path().file_name().unwrap().to_str().unwrap();
        let body = body_string(
            app_for_roots(&[a.path(), b.path()])
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The coverage label names what the readout spans, every root listed in
        // config order, comma-joined. The joined pair appears only in the label.
        assert!(body.contains("Coverage in"));
        assert!(body.contains(&format!("{a_name}, {b_name}")));
    }

    #[tokio::test]
    async fn gap_summary_renders_a_chip_per_root_for_a_multi_root_config() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        touch(&a.path().join("BookA/01.mp3"));
        touch(&b.path().join("BookB/01.mp3"));
        let body = body_string(
            app_for_roots(&[a.path(), b.path()])
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // One chip per root, each with its own gap count and a data-root hook the
        // client recompute updates.
        assert!(body.contains(r#"id="gap-chips""#));
        assert!(body.contains(r#"class="gap-chip" data-root="0""#));
        assert!(body.contains(r#"data-root="1""#));
    }

    #[tokio::test]
    async fn gap_summary_omits_chips_for_a_single_root() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(!body.contains(r#"id="gap-chips""#));
    }

    #[tokio::test]
    async fn gap_summary_chips_handle_a_clean_and_an_error_root() {
        let good = tempfile::tempdir().unwrap();
        touch(&good.path().join("Book/01.mp3"));
        touch(&good.path().join("Book/Book.epub")); // covered -> Clean
        let body = body_string(
            app_for_roots(&[good.path(), Path::new("/no/such/root/xyz123")])
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // Total is zero, so the all-clear message shows, and a multi-root setup still
        // gets its chips, the error root labelled.
        assert!(body.contains("All clear"));
        assert!(body.contains(r#"id="gap-chips""#));
        assert!(body.contains("gap-chip-clean"));
        assert!(body.contains("gap-chip-error"));
        assert!(body.contains("scan error"));
    }

    #[tokio::test]
    async fn gap_summary_renders_a_session_progressbar() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A progressbar that starts empty: this sitting's resolved-over-baseline meter.
        assert!(body.contains(r#"role="progressbar""#));
        assert!(body.contains(r#"aria-valuenow="0""#));
        assert!(body.contains(r#"aria-valuemax="1""#));
        assert!(body.contains(r#"aria-valuemin="0""#));
        assert!(body.contains(r#"id="gap-bar-fill""#));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_gap_summary_and_session_bar() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The strip, its chips, and the session bar are themed.
        assert!(body.contains(".gap-summary"));
        assert!(body.contains(".gap-chip"));
        assert!(body.contains(".gap-bar-fill"));
        // The fill animates its width, and the strip stacks on a phone.
        assert!(body.contains("transition: width"));
        assert!(body.contains(".gap-summary-head"));
        // The session coverage block is themed.
        assert!(body.contains(".gap-session"));
    }

    #[tokio::test]
    async fn navbar_renders_the_brand_mark_before_the_title() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The title is a home link wrapping the brand glyph and the wordmark. The
        // single assertion fixes the link, the inline mark, and its leading position.
        assert!(body.contains(r#"<h1><a href="/"><svg class="brand-mark""#));
        assert!(body.contains("Missing Ebooks"));
    }

    #[tokio::test]
    async fn navbar_places_the_spacer_before_the_search_box() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The flexible spacer sits right after the title, so the title alone pins to the
        // left and the search box groups with the controls on the right.
        let spacer = body
            .find(r#"<span class="spacer">"#)
            .expect("spacer present");
        let search = body
            .find(r#"<div class="search""#)
            .expect("search box present");
        assert!(
            spacer < search,
            "the spacer should sit before the search box"
        );
    }

    #[tokio::test]
    async fn navbar_renders_the_hidden_filter_input_and_no_matches_line() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A filter input with an accessible name, hidden until app.js reveals it (the
        // connection banner's pattern), so the no-JS page stays clean.
        assert!(body.contains(r#"id="search-input""#));
        assert!(body.contains(r#"aria-label="Filter folders""#));
        assert!(body.contains(r#"id="search" hidden"#));
        // A polite "no matches" line, hidden until a query matches nothing.
        assert!(body.contains(r#"id="search-empty""#));
        assert!(body.contains(r#"aria-live="polite""#));
        assert!(body.contains("No folders match"));
    }

    #[tokio::test]
    async fn search_box_renders_a_hidden_themed_clear_button() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A labelled clear button sits in the filter box, hidden at first paint so it
        // only appears once the box holds text (app.js drives the toggle).
        assert!(body.contains(r#"id="search-clear""#));
        assert!(body.contains(r#"aria-label="Clear filter" hidden"#));
        // It carries a thin-× glyph, not a circle: two diagonal strokes.
        assert!(body.contains(r#"d="M6 6l12 12M18 6L6 18""#));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_filter_input_and_hidden_states() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The filter input is styled and hidden until JS reveals it.
        assert!(body.contains(".search-input"));
        assert!(body.contains(".search[hidden]"));
        // Filtered-out branches collapse, and the input drops to its own navbar row on
        // a phone, the way the view toggle already reflows.
        assert!(body.contains(".filter-hidden"));
        assert!(body.contains(".navbar .search"));
    }

    #[tokio::test]
    async fn stylesheet_themes_the_clear_button_and_hides_the_native_one() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // Our themed clear button replaces the browser's native, un-themed cancel
        // control, which we hide.
        assert!(body.contains("::-webkit-search-cancel-button"));
        assert!(body.contains(".search-clear"));
        // It darkens on hover and, while the box is empty, stays in the layout but
        // invisible so its slot is reserved and the field width never changes.
        assert!(body.contains(".search-clear:hover"));
        assert!(body.contains(".search-clear[hidden]"));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_title_home_link() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The title link carries the brand-mark spacing, underlines on hover, and
        // shows a focus ring for keyboard users.
        assert!(body.contains(".navbar h1 a"));
        assert!(body.contains(".navbar h1 a:hover"));
        assert!(body.contains(".navbar h1 a:focus-visible"));
    }

    #[tokio::test]
    async fn index_renders_the_shortcuts_inside_the_settings_panel() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The shortcuts are a read-only section inside the settings popover now, so
        // the standalone cheatsheet dialog and its navbar trigger are gone.
        assert!(!body.contains(r#"id="cheatsheet""#));
        assert!(!body.contains(r#"id="cheatsheet-btn""#));
        assert!(body.contains(r#"class="settings-shortcuts""#));
        assert!(body.contains("Keyboard shortcuts"));
        // The keys are spelled out for the reader.
        assert!(body.contains("<kbd>j</kbd>"));
        assert!(body.contains("Move between gaps"));
        // Enter leaves the filter box, the complement of / focusing it.
        assert!(body.contains("<kbd>Enter</kbd>"));
        assert!(body.contains("Exit the filter"));
        // The mark shortcuts were removed, so the list no longer mentions them.
        assert!(!body.contains("Mark as no ebook"));
        assert!(!body.contains("Mark ebook elsewhere"));
    }

    #[tokio::test]
    async fn app_script_opens_the_settings_popover_for_the_help_key() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The `?` key opens the merged settings popover; the old cheatsheet helper
        // is gone.
        assert!(body.contains("showPopover"));
        assert!(!body.contains("openCheatsheet"));
    }

    #[tokio::test]
    async fn app_script_reveals_and_runs_the_filter() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The filter reveals the hidden input, recurses the tree, toggles the
        // collapse class on non-matching branches, and shows the "no matches" line.
        assert!(body.contains("filterTree"));
        assert!(body.contains("filter-hidden"));
        assert!(body.contains("search-empty"));
        assert!(body.contains("clearFilter"));
        // Enter in the box drops focus, so the live filter stays but the keyboard
        // returns to navigation.
        assert!(body.contains(r#"evt.key === "Enter""#));
        // The live filter rides the view-toggle link as a q param and is re-applied
        // from the URL on the next page, so switching views keeps the filter.
        assert!(body.contains("syncViewLink"));
        assert!(body.contains("URLSearchParams"));
    }

    #[tokio::test]
    async fn app_script_toggles_and_handles_the_clear_button() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The script finds the clear button and toggles its visibility from the input
        // value, so it shows only when the box holds text.
        assert!(body.contains("search-clear"));
        assert!(body.contains("toggleClear"));
    }

    #[tokio::test]
    async fn index_tolerates_a_filter_query_param_on_a_view_switch() {
        // The client carries the live filter across a view switch as a q param; the
        // server has no use for it and must ignore it, not reject the request.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/?view=all&q=Book")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains(r#"id="roots""#));
    }

    #[tokio::test]
    async fn app_script_recomputes_the_summary_and_session_bar() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The summary is recomputed from the DOM as marks land, and the session bar
        // tracks resolved-over-baseline, with the baseline seeded from the strip's
        // data hook and reset on a rescan.
        assert!(body.contains("recomputeSummary"));
        assert!(body.contains("sessionBaseline"));
        assert!(body.contains("gap-bar-fill"));
        assert!(body.contains("gapsAtLoad"));
        // It runs on a confirmed mark, on an undo/section swap, and on a rescan.
        assert!(body.contains(r#"addEventListener("marked""#));
        assert!(body.contains("htmx:afterSwap"));
        // The readout numbers track the bar: resolved of baseline, and the percent.
        assert!(body.contains("gap-resolved"));
        assert!(body.contains("gap-pct"));
    }

    #[tokio::test]
    async fn app_script_defines_the_hotkeys_and_active_row() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // j/k move a focusable highlight through the visible gap rows; r rescans;
        // / focuses the filter; ? opens the cheatsheet; Escape clears or drops.
        assert!(body.contains("moveHighlight"));
        assert!(body.contains("visibleGapRows"));
        assert!(body.contains("row-active"));
        // The mark hotkeys were removed, so the row-marking helper is gone too.
        assert!(!body.contains("markActiveRow"));
        // Keys are ignored while typing in a field.
        assert!(body.contains("isEditable"));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_active_row_highlight() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The j/k highlight is a real focus target: a tinted band and a focus ring.
        assert!(body.contains(".row-active"));
        assert!(body.contains("outline"));
    }
}
