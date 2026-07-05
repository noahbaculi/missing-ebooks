//! axum router, request handlers, and Maud markup. Handlers are thin: call a
//! `service` operation and render. They return `Html<String>` so Maud stays
//! decoupled from the axum version. Marker writes use htmx to swap only the
//! affected root's section. htmx is vendored and served from `/static`.

use std::convert::Infallible;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderName, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use maud::Markup;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::marker::Marker;
use crate::state::{AppState, WriteFailure};
use crate::tree::ViewMode;

pub(crate) mod assets;
mod page;
pub mod render;

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
    /// Raw `view` form field, parsed leniently via `ViewMode::from_query` at the
    /// call site so an unknown value falls back to gaps-only rather than 422,
    /// matching the index/rescan handlers.
    #[serde(default)]
    pub(crate) view: Option<String>,
}

/// Build the application router with the shared state attached.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/mark", post(mark))
        .route("/unmark", post(unmark))
        .route("/rescan", post(rescan))
        .route("/events", get(events))
        .route("/static/htmx.min.js", get(assets::htmx_script))
        .route("/static/htmx-sse.js", get(assets::htmx_sse_script))
        .route("/static/app.css", get(assets::app_css))
        .route("/static/app.js", get(assets::app_js))
        .with_state(state)
}

async fn index(State(state): State<Arc<AppState>>, Query(query): Query<ViewQuery>) -> Html<String> {
    let started = Instant::now();
    let mode = ViewMode::from_query(query.view.as_deref());
    let raw = state.store.current().await;
    let render_started = Instant::now();
    let html = render::page(&raw, &state.config.search_links, mode).into_string();
    tracing::debug!(
        op = "index",
        mode = mode.as_query(),
        render_ms = render_started.elapsed().as_secs_f64() * 1e3,
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "handled request"
    );
    Html(html)
}

async fn mark(
    State(state): State<Arc<AppState>>,
    Form(req): Form<MarkRequest>,
) -> axum::response::Response {
    let started = Instant::now();
    let links = &state.config.search_links;
    let mode = ViewMode::from_query(req.view.as_deref());
    let resp = match state.store.write_mark(req.root, &req.rel, req.kind).await {
        Ok(applied) => {
            let handle = render::packaged_section(&applied.raw, req.root, mode);
            let markup = handle.render(links, None);
            let trigger = applied.created.then(|| {
                // Read the section path off the packaged raw scan so the
                // toast's display name still comes from the same source
                // it used before the fold.
                let section_path = applied.raw[req.root].display_path().to_string();
                let name = display_name(&section_path, &req.rel);
                marked_trigger(&req, &name)
            });
            section_response(markup, trigger)
        }
        Err(WriteFailure::BadRoot) => bad_root_response(req.root, &req.rel, "mark"),
        Err(WriteFailure::Failed { error, raw }) => {
            let handle = render::packaged_section(&raw, req.root, mode);
            let message = format!("Could not mark {}: {error}", req.rel);
            section_response(handle.render(links, Some(&message)), None)
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

async fn unmark(
    State(state): State<Arc<AppState>>,
    Form(req): Form<MarkRequest>,
) -> axum::response::Response {
    let started = Instant::now();
    let links = &state.config.search_links;
    let mode = ViewMode::from_query(req.view.as_deref());
    let resp = match state.store.remove_mark(req.root, &req.rel, req.kind).await {
        Ok(raw) => {
            let handle = render::packaged_section(&raw, req.root, mode);
            section_response(handle.render(links, None), None)
        }
        Err(WriteFailure::BadRoot) => bad_root_response(req.root, &req.rel, "undo"),
        Err(WriteFailure::Failed { error, raw }) => {
            let handle = render::packaged_section(&raw, req.root, mode);
            let message = format!("Could not undo {}: {error}", req.rel);
            section_response(handle.render(links, Some(&message)), None)
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

/// Render the standalone error card. Used by `mark` and `unmark` for the
/// `WriteFailure::BadRoot` arm, where the submitted root index is out of
/// range and there is no section to render the alert into.
fn bad_root_response(root: usize, rel: &str, op: &str) -> axum::response::Response {
    let message = format!("Could not {op} {rel}: no such library root");
    section_response(render::error_section(root, &message), None)
}

async fn rescan(State(state): State<Arc<AppState>>, Form(query): Form<ViewQuery>) -> Response {
    let started = Instant::now();
    let mode = ViewMode::from_query(query.view.as_deref());
    let raw = state.store.rescan().await;
    // Swap the fresh sections into #roots and push the mode path, so the address bar
    // tracks the view without ever showing the /rescan POST URL.
    let markup = render::all_sections(
        &raw,
        &state.config.search_links,
        mode,
        render::SectionWrap::Plain,
    );
    let resp = ([("HX-Push-Url", mode.path())], Html(markup.into_string())).into_response();
    tracing::debug!(
        op = "rescan",
        mode = mode.as_query(),
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "handled request"
    );
    resp
}

/// Wrap an SSE receiver into an axum `Response` with the `X-Accel-Buffering: no`
/// header set so nginx (and other reverse proxies that respect this header) do
/// not buffer the stream. axum's `Sse::into_response` sets `Content-Type:
/// text/event-stream` and `Cache-Control: no-cache` but not this one. Shared
/// by the production and demo `/events` handlers so they cannot drift.
pub(crate) fn events_response(rx: mpsc::Receiver<Result<Event, Infallible>>) -> Response {
    let sse = Sse::new(ReceiverStream::new(rx)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    );
    let mut response = sse.into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

/// The SSE event sent first on every `/events` connection. Carries `event: ack`,
/// `id: r`, and empty data. No client listens to `ack`. Its sole purpose is to
/// seed the browser's `lastEventId` so any reconnect carries `Last-Event-ID`,
/// which `events` uses to discriminate first connect from reconnect. See
/// ADR-0030.
#[allow(
    clippy::unnecessary_wraps,
    reason = "SSE senders take Result<Event, Infallible>; the wrap lets callers do tx.send(ack_event()) directly."
)]
pub(crate) fn ack_event() -> Result<Event, Infallible> {
    Ok(Event::default().event("ack").id("r"))
}

/// The SSE `snapshot` event. The `id: r` stamp is identical to every other
/// event on the channel. The server only checks header presence on reconnect,
/// not the id value. See ADR-0030.
#[allow(
    clippy::unnecessary_wraps,
    reason = "SSE senders take Result<Event, Infallible>; the wrap lets callers do tx.send(snapshot_event(...)) directly."
)]
pub(crate) fn snapshot_event(payload: String) -> Result<Event, Infallible> {
    Ok(Event::default().event("snapshot").id("r").data(payload))
}

/// The per-autosync-tick `section` event. Same `id: r` stamp as the rest of
/// the channel for the same reason as `snapshot_event`.
pub(crate) fn section_event(html: String) -> Event {
    Event::default().event("section").id("r").data(html)
}

/// SSE endpoint. The first event is always `ack`, an id-stamped sentinel that
/// seeds the browser's `lastEventId` so a future reconnect carries
/// `Last-Event-ID`. The second event is `snapshot` only when the request
/// already carries `Last-Event-ID`: presence means the browser is reconnecting
/// after a drop and the snapshot fills the gap. Absence means first connect,
/// when the page just rendered the same state inline. Subsequent events are
/// `section` events from the autosync loop. `ping` events come from
/// `KeepAlive` every 15 seconds to survive idle TCP drops by reverse proxies.
/// See ADR-0030.
async fn events(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<ViewQuery>,
) -> Response {
    let mode = ViewMode::from_query(query.view.as_deref());
    // Last-Event-ID is set by the browser's EventSource on any reconnect after
    // it has received at least one id'd event. Presence means reconnect, so
    // the snapshot is needed to catch the client up. Absence means first
    // connect. The page just rendered the same state inline.
    let send_snapshot = headers.contains_key("last-event-id");
    let rx = crate::autosync::attach(&state, mode, send_snapshot).await;
    events_response(rx)
}

/// JSON-escape any non-ASCII char to `\uXXXX` so an `HX-Trigger` value stays pure
/// ASCII and survives browser header decoding. Replacing a char with its escape
/// keeps valid JSON valid. Folder names are often non-ASCII, so this is the common
/// path, not an edge case.
fn ascii_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u16; 2];
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut buf) {
                // Header values are ASCII, and write! into a String is infallible.
                let _ = write!(out, "\\u{unit:04x}");
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
            "view": ViewMode::from_query(req.view.as_deref()).as_query(),
            "name": name,
        }
    });
    ascii_escape(&payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::config::Config;
    use crate::scanner::ScanSettings;
    use crate::scenarios::touch;

    fn app_for(root: &Path) -> Router {
        let cfg = Config {
            library_roots: vec![root.to_path_buf()],
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
    async fn rescan_without_htmx_still_returns_sections() {
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
        // There is no no-JS redirect: every rescan renders the sections and pushes
        // the view URL, whether or not the request came from htmx.
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("HX-Push-Url").unwrap(), "/");
        let body = body_string(response).await;
        assert!(body.contains(r#"class="card root""#));
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
    async fn mark_response_carries_section_with_total_audiobooks() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("A/B1/01.mp3"));
        touch(&dir.path().join("A/B2/01.mp3"));
        // Mark B1 as no-ebook. The response is the re-rendered section.
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=A%2FB1&kind=no_ebook"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        // The mark response is the re-rendered section. The total rides along
        // unchanged because a scan total does not shift mid-mark.
        assert!(body.contains(r#"data-total-audiobooks="2""#));
    }

    #[tokio::test]
    async fn index_tolerates_a_filter_query_param_on_a_view_switch() {
        // The client carries the live filter across a view switch as a q param.
        // The server has no use for it and must ignore it, not reject the request.
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

    #[test]
    fn ack_event_carries_event_name_and_id_for_last_event_id_seeding() {
        let event = ack_event().unwrap();
        let rendered = format!("{event:?}");
        assert!(
            rendered.contains("event: ack"),
            "ack_event must carry event: ack, got {rendered}"
        );
        assert!(
            rendered.contains("id: r"),
            "ack_event must carry id: r, got {rendered}"
        );
    }

    #[test]
    fn snapshot_event_carries_event_name_and_id_and_payload() {
        let event = snapshot_event("<oob>payload</oob>".to_string()).unwrap();
        let rendered = format!("{event:?}");
        assert!(rendered.contains("event: snapshot"));
        assert!(rendered.contains("id: r"));
        assert!(rendered.contains("payload"));
    }

    #[test]
    fn section_event_carries_event_name_and_id_and_payload() {
        let event = section_event("<oob>section</oob>".to_string());
        let rendered = format!("{event:?}");
        assert!(rendered.contains("event: section"));
        assert!(rendered.contains("id: r"));
    }

    #[tokio::test]
    async fn mark_tolerates_an_unknown_view_value() {
        // An unknown view= must fall back to gaps-only, matching index/rescan,
        // rather than 422 on the strict ViewMode deserialize it used to do.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=bogus"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_failed_root_scan_renders_the_error_banner_not_a_500() {
        // A root that exists at construction (so it would survive startup
        // Config::validate) but cannot be walked when the first request triggers
        // the lazy scan: remove it after the app is built. canonicalize then
        // fails, the scanner yields RootScan::Failed, and the section must render
        // its error banner with a graceful 200. Portable: no chmod, no
        // non-directory trickery.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        std::fs::create_dir(&root).unwrap();
        let app = app_for(&root);
        std::fs::remove_dir(&root).unwrap();

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a failed root scan must not 500",
        );
        let body = body_string(response).await;
        assert!(
            body.contains("Could not scan this root:"),
            "the failed root must render its error banner",
        );
    }
}
