//! axum router, request handlers, and Maud markup. Handlers are thin: they call a
//! `service` operation and render. Handlers return `Html<String>` so Maud stays
//! decoupled from the axum version. Marker writes use htmx to swap just the
//! affected root's section; the script is vendored and served from `/static`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Form, State};
use axum::http::header;
use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::{get, post};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde::Deserialize;

use crate::config::SearchLink;
use crate::marker::Marker;
use crate::query::clean_query;
use crate::service::{self, FlaggedView, RootSection, RootState};
use crate::state::AppState;
use crate::tree::Node;

/// The vendored htmx runtime, embedded at compile time and served from /static.
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

const PAGE_CSS: &str = "\
body { font-family: system-ui, sans-serif; margin: 2rem; max-width: 60rem; }
h2 { font-size: 1rem; color: #333; word-break: break-all; }
ul.tree { list-style: none; padding-left: 1rem; margin: 0.2rem 0; }
li.node { margin: 0.1rem 0; }
summary { cursor: pointer; }
.flagged { font-weight: 600; }
.rel { color: #777; margin-left: 0.5rem; font-size: 0.85em; }
.clean { color: #555; font-style: italic; }
.error { color: #b00000; }
form.mark { display: inline; margin-left: 0.5rem; }
form.mark button { font-size: 0.75em; margin-left: 0.25rem; cursor: pointer; }
span.links { margin-left: 0.5rem; }
span.links a { font-size: 0.75em; margin-left: 0.25rem; }
";

/// The body of a marker write: which root, which folder, and which marker.
#[derive(Deserialize)]
struct MarkRequest {
    root: usize,
    rel: String,
    kind: Marker,
}

/// Build the application router with the shared state attached.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/mark", post(mark))
        .route("/rescan", post(rescan))
        .route("/static/htmx.min.js", get(htmx_script))
        .with_state(state)
}

async fn index(State(state): State<Arc<AppState>>) -> Html<String> {
    let view = service::current_view(&state).await;
    Html(page(&view, &state.config.search_links).into_string())
}

async fn mark(State(state): State<Arc<AppState>>, Form(req): Form<MarkRequest>) -> Html<String> {
    let links = &state.config.search_links;
    match service::mark(&state, req.root, &req.rel, req.kind).await {
        Ok(view) => Html(render_section(&view[req.root], req.root, None, links).into_string()),
        Err(err) => {
            let message = format!("Could not mark {}: {err}", req.rel);
            let view = service::current_view(&state).await;
            let markup = match view.get(req.root) {
                Some(section) => render_section(section, req.root, Some(&message), links),
                None => html! { section.root { p.error { (message) } } },
            };
            Html(markup.into_string())
        }
    }
}

async fn rescan(State(state): State<Arc<AppState>>) -> Redirect {
    service::rescan(&state).await;
    // 303 See Other: Post/Redirect/Get, so a refresh does not re-trigger a scan.
    Redirect::to("/")
}

async fn htmx_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript;charset=utf-8")],
        HTMX_JS,
    )
}

fn page(view: &FlaggedView, links: &[SearchLink]) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Missing Ebooks" }
                style { (PreEscaped(PAGE_CSS)) }
            }
            body {
                h1 { "Missing Ebooks" }
                form method="post" action="/rescan" {
                    button type="submit" { "Rescan" }
                }
                @for (root, section) in view.iter().enumerate() {
                    (render_section(section, root, None, links))
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
) -> Markup {
    html! {
        section.root {
            h2 { (section.path) }
            @if let Some(message) = error {
                p.error { (message) }
            }
            @match &section.state {
                RootState::Forest(nodes) => {
                    ul.tree {
                        @for node in nodes { (render_node(node, root, links)) }
                    }
                }
                RootState::Clean => {
                    p.clean { "No missing ebooks in this root" }
                }
                RootState::Error(message) => {
                    p.error { "Could not scan this root: " (message) }
                }
            }
        }
    }
}

fn render_node(node: &Node, root: usize, links: &[SearchLink]) -> Markup {
    html! {
        @if node.children.is_empty() {
            li.node.flagged[node.flagged] {
                span.name { (node.name) }
                span.rel { (node.rel_path) }
                (marker_buttons(root, &node.rel_path))
                (search_links(links, &node.name))
            }
        } @else {
            li.node {
                details open {
                    summary.flagged[node.flagged] {
                        span.name { (node.name) }
                        span.rel { (node.rel_path) }
                        (marker_buttons(root, &node.rel_path))
                        (search_links(links, &node.name))
                    }
                    ul.tree {
                        @for child in &node.children { (render_node(child, root, links)) }
                    }
                }
            }
        }
    }
}

fn marker_buttons(root: usize, rel: &str) -> Markup {
    html! {
        form.mark hx-target="closest section.root" hx-swap="outerHTML" {
            input type="hidden" name="root" value=(root);
            input type="hidden" name="rel" value=(rel);
            button type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"no_ebook"}"#)
                onclick="event.stopPropagation()" { "No ebook" }
            button type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"ebook_elsewhere"}"#)
                onclick="event.stopPropagation()" { "Ebook elsewhere" }
        }
    }
}

fn search_links(links: &[SearchLink], name: &str) -> Markup {
    html! {
        @if !links.is_empty() {
            @let query = urlencoding::encode(&clean_query(name)).into_owned();
            span.links {
                @for link in links {
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
    use crate::scanner::{ScanInputs, ScanSettings};

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    fn app_for(root: &Path) -> Router {
        let cfg = Config {
            library_roots: vec![root.to_path_buf()],
            ttl_seconds: 60,
            ..Default::default()
        };
        let defaults = Config::default();
        let settings = ScanSettings::compile(ScanInputs {
            audio_exts: &defaults.audio_exts,
            ebook_exts: &defaults.ebook_exts,
            excluded_dirs: &[],
            exclude_globs: &[],
        })
        .unwrap();
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
                    .body(Body::empty())
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
}
