//! axum router, request handlers, and Maud markup for the read-only UI. Handlers
//! are thin: they call a `service` operation and render. Handlers return
//! `Html<String>` so Maud stays decoupled from the axum version.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::{Html, Redirect};
use axum::routing::{get, post};
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::service::{self, FlaggedView, RootSection, RootState};
use crate::state::AppState;
use crate::tree::Node;

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
";

/// Build the application router with the shared state attached.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/rescan", post(rescan))
        .with_state(state)
}

async fn index(State(state): State<Arc<AppState>>) -> Html<String> {
    let view = service::current_view(&state).await;
    Html(page(&view).into_string())
}

async fn rescan(State(state): State<Arc<AppState>>) -> Redirect {
    service::rescan(&state).await;
    // 303 See Other: Post/Redirect/Get, so a refresh does not re-trigger a scan.
    Redirect::to("/")
}

fn page(view: &FlaggedView) -> Markup {
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
                @for section in view {
                    (render_section(section))
                }
            }
        }
    }
}

fn render_section(section: &RootSection) -> Markup {
    html! {
        section.root {
            h2 { (section.path) }
            @match &section.state {
                RootState::Forest(nodes) => {
                    ul.tree {
                        @for node in nodes { (render_node(node)) }
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

fn render_node(node: &Node) -> Markup {
    html! {
        @if node.children.is_empty() {
            li.node.flagged[node.flagged] {
                span.name { (node.name) }
                span.rel { (node.rel_path) }
            }
        } @else {
            li.node {
                details open {
                    summary.flagged[node.flagged] {
                        span.name { (node.name) }
                        span.rel { (node.rel_path) }
                    }
                    ul.tree {
                        @for child in &node.children { (render_node(child)) }
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
}
