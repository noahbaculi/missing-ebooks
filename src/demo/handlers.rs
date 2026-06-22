//! The demo's axum router and handlers. Handlers reuse the production `page`,
//! `render_section`, `apply_mark_raw`, and `render_view` from the library, plus
//! the static-asset handlers. A visitor is pinned to an in-memory session by a
//! cookie; their marks are replayed on top of the shared raw view per request,
//! then rendered for the requested mode. The full index page carries the demo
//! banner; the `/mark` partial does not.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::Event;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use tokio::sync::mpsc;

use crate::service::{FlaggedView, ViewMode, apply_mark_raw, render_view};
use crate::state::RawView;
use crate::web::render::oob_sections;
use crate::web::{
    MarkRequest, ViewQuery, app_css, events_response, htmx_script, htmx_sse_script, page,
    render_section,
};

use super::banner;
use super::session::{AtCapacity, Mark, SessionId, SessionStore};
use super::state::{DemoConfig, DemoState};

/// The page shown when the global session cap is reached. Served with HTTP 503 so
/// bots and monitors read it as a soft, retryable refusal. Self-contained so it
/// needs no stylesheet.
const CAPACITY_HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Missing Ebooks demo is busy</title></head>
<body style="font:16px/1.5 system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem">
<h1>The demo is at capacity</h1>
<p>Every demo session is in use right now. Each one is a throwaway view that
frees up after a few idle minutes. Please try again shortly.</p>
</body></html>"#;

/// Build the demo router over shared state. `/healthz` answers without touching
/// the session table, so the container healthcheck never mints a session.
pub fn router(state: Arc<DemoState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/mark", post(mark))
        .route("/reset", post(reset))
        .route("/rescan", post(rescan))
        .route("/events", get(events))
        .route("/healthz", get(healthz))
        .route("/static/htmx.min.js", get(htmx_script))
        .route("/static/htmx-sse.js", get(htmx_sse_script))
        .route("/static/app.css", get(app_css))
        .with_state(state)
}

/// Pull the session id out of the `Cookie` header for `cookie_name`.
fn read_cookie(headers: &HeaderMap, cookie_name: &str) -> Option<SessionId> {
    let raw = headers.get("cookie")?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        // Strip the name then the `=` separately, so a name that only shares a
        // prefix (for example `me_demo_sidX=`) still fails to match.
        if let Some(rest) = pair.strip_prefix(cookie_name)
            && let Some(value) = rest.strip_prefix('=')
            && !value.is_empty()
        {
            return Some(SessionId(value.to_string()));
        }
    }
    None
}

/// Lowercase hex digits, indexed by nibble to render a session id.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Mint a new random session id as 32 hex characters.
fn new_session_id() -> SessionId {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("OS rng");
    let mut out = String::with_capacity(32);
    for &b in &buf {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    SessionId(out)
}

/// Build the `Set-Cookie` value for a new session, scoped to the whole site and
/// expiring with the idle window. `Secure` is set because the demo is reached
/// only over HTTPS at the Cloudflare edge.
fn cookie_header(cookie_name: &str, sid: &SessionId, max_age_secs: u64) -> HeaderValue {
    let value = format!(
        "{cookie_name}={}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age_secs}",
        sid.0
    );
    HeaderValue::from_str(&value).expect("ascii cookie")
}

/// Resolve the request's session against an already-locked store, minting one
/// when the cookie is absent or unknown. Returns the id and an optional
/// Set-Cookie (present only when a session was freshly minted), or `None` when
/// the store is at capacity and the visitor has no live session.
fn resolve_in_store(
    store: &mut SessionStore,
    config: &DemoConfig,
    existing: Option<SessionId>,
    now: Instant,
) -> Option<(SessionId, Option<HeaderValue>)> {
    if let Some(sid) = existing
        && store.touch(&sid, now)
    {
        return Some((sid, None));
    }
    let sid = new_session_id();
    match store.create(sid.clone(), now) {
        Ok(()) => {
            let cookie = cookie_header(&config.cookie_name, &sid, config.idle.as_secs());
            Some((sid, Some(cookie)))
        }
        Err(AtCapacity) => None,
    }
}

/// Derive one session's rendered view for a mode: clone the shared raw view,
/// replay every session mark via `apply_mark_raw`, then render. With no marks
/// the clone-and-render still runs because per-request rendering is the new
/// baseline; the per-request cost is bounded (see ADR-0022). A mark naming an
/// out-of-range root is skipped defensively; an unmatched `rel` is a no-op
/// inside `apply_mark_raw`.
fn derive_view(base: &RawView, marks: &[Mark], mode: ViewMode) -> FlaggedView {
    if marks.is_empty() {
        return render_view(base, mode);
    }
    let mut raw = base.clone();
    for mark in marks {
        if mark.root >= raw.len() {
            continue;
        }
        apply_mark_raw(&mut raw, mark.root, &mark.rel, mark.kind);
    }
    render_view(&raw, mode)
}

/// The 503 at-capacity response.
fn capacity_response() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Html(CAPACITY_HTML)).into_response()
}

async fn index(
    State(state): State<Arc<DemoState>>,
    headers: HeaderMap,
    Query(query): Query<ViewQuery>,
) -> Response {
    let mode = ViewMode::from_query(query.view.as_deref());
    let now = Instant::now();
    let existing = read_cookie(&headers, &state.config.cookie_name);
    let resolved = {
        let mut store = state.sessions.lock().expect("session lock");
        resolve_in_store(&mut store, &state.config, existing, now)
            .map(|(sid, set_cookie)| (set_cookie, store.marks(&sid).to_vec()))
    };
    let Some((set_cookie, marks)) = resolved else {
        return capacity_response();
    };
    let view = derive_view(state.base_raw(), &marks, mode);
    let html = page(&view, &state.search_links, mode).into_string();
    let mut response = Html(banner::inject(&html, mode)).into_response();
    if let Some(cookie) = set_cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

async fn mark(
    State(state): State<Arc<DemoState>>,
    headers: HeaderMap,
    Form(req): Form<MarkRequest>,
) -> Response {
    let mode = req.view;
    // The UI only ever submits a root index from a rendered button, so an
    // out-of-range index is a malformed request.
    if req.root >= state.num_roots() {
        return (StatusCode::BAD_REQUEST, "unknown library root").into_response();
    }
    let now = Instant::now();
    let existing = read_cookie(&headers, &state.config.cookie_name);
    let resolved = {
        let mut store = state.sessions.lock().expect("session lock");
        match resolve_in_store(&mut store, &state.config, existing, now) {
            Some((sid, set_cookie)) => {
                store.append_mark(
                    &sid,
                    Mark {
                        root: req.root,
                        rel: req.rel.clone(),
                        kind: req.kind,
                    },
                );
                Some((set_cookie, store.marks(&sid).to_vec()))
            }
            None => None,
        }
    };
    let Some((set_cookie, marks)) = resolved else {
        return capacity_response();
    };
    let view = derive_view(state.base_raw(), &marks, mode);
    let markup = render_section(&view[req.root], req.root, None, &state.search_links, mode);
    let mut response = Html(markup.into_string()).into_response();
    if let Some(cookie) = set_cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

/// Redirect to the page for `mode`. Used after a POST so a refresh re-issues a
/// GET instead of re-firing the action (Post/Redirect/Get). `Redirect::to` emits
/// 303 See Other.
fn redirect_to_view(mode: ViewMode) -> Redirect {
    match mode {
        ViewMode::GapsOnly => Redirect::to("/"),
        ViewMode::All => Redirect::to("/?view=all"),
    }
}

async fn reset(
    State(state): State<Arc<DemoState>>,
    headers: HeaderMap,
    Form(query): Form<ViewQuery>,
) -> Response {
    let mode = ViewMode::from_query(query.view.as_deref());
    let now = Instant::now();
    let existing = read_cookie(&headers, &state.config.cookie_name);
    let set_cookie = {
        let mut store = state.sessions.lock().expect("session lock");
        match resolve_in_store(&mut store, &state.config, existing, now) {
            Some((sid, set_cookie)) => {
                store.clear_marks(&sid);
                set_cookie
            }
            // At capacity with no live session: the same soft 503 the others serve.
            None => return capacity_response(),
        }
    };
    // Back to the view the reset came from, so a refresh lands the visitor where
    // they were rather than re-firing the reset.
    let mut response = redirect_to_view(mode).into_response();
    if let Some(cookie) = set_cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

async fn rescan(Form(query): Form<ViewQuery>) -> Redirect {
    // Every GET re-derives, and marks live in the session, so a rescan just
    // returns the visitor to their current view.
    let mode = ViewMode::from_query(query.view.as_deref());
    redirect_to_view(mode)
}

/// The demo's `/events` endpoint. Serves the per-session snapshot once, then
/// keeps the connection alive with pings so the page's SSE listener never 404s
/// and never reconnects in a loop. The demo runs no autosync loop (its library
/// is static and its marks are in-process), so no `section` events are ever
/// emitted (ADR-0023). A follow-up captures showcasing autosync in the demo.
async fn events(
    State(state): State<Arc<DemoState>>,
    headers: HeaderMap,
    Query(query): Query<ViewQuery>,
) -> Response {
    let mode = ViewMode::from_query(query.view.as_deref());
    let now = Instant::now();
    let existing = read_cookie(&headers, &state.config.cookie_name);
    let marks = {
        let mut store = state.sessions.lock().expect("session lock");
        // resolve_in_store may fail under capacity; the SSE channel just sends
        // an empty snapshot in that case, since the page's other GET would
        // already have shown the capacity response.
        resolve_in_store(&mut store, &state.config, existing, now)
            .map(|(sid, _set_cookie)| store.marks(&sid).to_vec())
            .unwrap_or_default()
    };
    let view = derive_view(state.base_raw(), &marks, mode);
    let snapshot = oob_sections(&view, &state.search_links, mode).into_string();

    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(4);
    let _ = tx
        .send(Ok(Event::default().event("snapshot").data(snapshot)))
        .await;

    events_response(rx)
}

/// Liveness probe for the container healthcheck. Answers without minting a
/// session.
async fn healthz() -> StatusCode {
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::config::Config;
    use crate::demo::state::build_state;
    use crate::marker::Marker;
    use crate::scanner::ScanSettings;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"").unwrap();
    }

    /// Build demo state over a single seeded root with the given cap and idle.
    async fn build(root: &Path, max_sessions: usize, idle: Duration) -> Arc<DemoState> {
        let cfg = Config {
            library_roots: vec![root.to_path_buf()],
            ttl_seconds: 60,
            ..Default::default()
        };
        let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
        let demo_config = DemoConfig {
            bind: "127.0.0.1:0".to_string(),
            scenario: "test".to_string(),
            max_sessions,
            idle,
            cookie_name: "me_demo_sid".to_string(),
        };
        Arc::new(build_state(cfg, settings, demo_config).await)
    }

    async fn body_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// The leading `name=value` token of the response's Set-Cookie, ready to send
    /// back as a `Cookie` header.
    fn session_cookie(response: &Response) -> String {
        let set = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        set.split(';').next().unwrap().to_string()
    }

    #[tokio::test]
    async fn a_first_visit_sets_a_session_cookie() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 10, Duration::from_secs(1200)).await;
        let response = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let set = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set.contains("me_demo_sid="));
        assert!(set.contains("HttpOnly"));
        assert!(set.contains("SameSite=Lax"));
    }

    #[tokio::test]
    async fn a_mark_persists_across_requests_within_a_session() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 10, Duration::from_secs(1200)).await;

        let first = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let cookie = session_cookie(&first);

        let marked = router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(marked.status(), StatusCode::OK);

        let after = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(after).await;
        assert!(body.contains("No missing ebooks in this root"));
    }

    #[tokio::test]
    async fn a_second_session_does_not_see_the_first_sessions_marks() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 10, Duration::from_secs(1200)).await;

        let first = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let cookie = session_cookie(&first);
        router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();

        // A fresh visitor with no cookie gets a new session and still sees the gap.
        let fresh = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(fresh).await;
        assert!(body.contains("Book"));
        assert!(body.contains(r#"hx-post="/mark""#));
    }

    #[tokio::test]
    async fn the_index_carries_the_demo_banner_but_the_partial_does_not() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 10, Duration::from_secs(1200)).await;

        let index = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let cookie = session_cookie(&index);
        let body = body_string(index).await;
        assert!(body.contains("me-demo-banner"));

        let partial = router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let partial_body = body_string(partial).await;
        assert!(!partial_body.contains("me-demo-banner"));
    }

    #[tokio::test]
    async fn at_capacity_serves_the_503_page() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 1, Duration::from_secs(1200)).await;

        let first = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_string(second).await;
        assert!(body.contains("at capacity"));
    }

    #[tokio::test]
    async fn reset_clears_a_sessions_marks() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 10, Duration::from_secs(1200)).await;

        let first = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let cookie = session_cookie(&first);

        // Mark the only book, which drops it from the gaps view.
        router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Reset clears the mark and redirects to the gaps view.
        let reset = router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reset")
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::SEE_OTHER);
        assert_eq!(reset.headers().get("location").unwrap(), "/");

        // The book is flagged again on the next load.
        let after = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(after).await;
        assert!(body.contains("Book"));
        assert!(body.contains(r#"hx-post="/mark""#));
    }

    #[tokio::test]
    async fn reset_preserves_the_all_view_and_sets_a_cookie_for_a_new_visitor() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 10, Duration::from_secs(1200)).await;

        // A cookie-less reset mints a session and keeps the all view.
        let reset = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reset")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::SEE_OTHER);
        assert_eq!(reset.headers().get("location").unwrap(), "/?view=all");
        assert!(reset.headers().get(header::SET_COOKIE).is_some());
    }

    #[tokio::test]
    async fn reset_at_capacity_serves_the_503_like_the_other_posts() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 1, Duration::from_secs(1200)).await;

        // Fill the single slot with one visitor.
        let first = router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        // A cookie-less reset cannot mint a session in a full store, so it gets
        // the same soft 503 as a fresh GET rather than a redirect.
        let reset = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reset")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_string(reset).await;
        assert!(body.contains("at capacity"));
    }

    #[tokio::test]
    async fn the_reaper_drops_an_idle_session() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let idle = Duration::from_secs(1200);
        let state = build(dir.path(), 10, idle).await;

        router(state.clone())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let future = Instant::now() + idle + Duration::from_secs(1);
        assert_eq!(state.reap_idle(future), 1);
        assert_eq!(state.reap_idle(future), 0);
    }

    #[tokio::test]
    async fn derive_view_with_no_marks_matches_render_view_of_the_base() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = build(dir.path(), 10, Duration::from_secs(1200)).await;

        let plain = render_view(state.base_raw(), ViewMode::GapsOnly);
        let derived = derive_view(state.base_raw(), &[], ViewMode::GapsOnly);
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            serde_json::to_value(&derived).unwrap(),
            "with no marks, derive_view must match a direct render"
        );

        let marks = [Mark {
            root: 0,
            rel: "Book".to_string(),
            kind: Marker::NoEbook,
        }];
        let after = derive_view(state.base_raw(), &marks, ViewMode::GapsOnly);
        let after_json = serde_json::to_value(&after).unwrap();
        let plain_json = serde_json::to_value(&plain).unwrap();
        assert_ne!(
            after_json, plain_json,
            "replaying a mark must change the view"
        );
    }

    #[test]
    fn new_session_id_is_32_lowercase_hex_chars() {
        let id = new_session_id().0;
        assert_eq!(id.len(), 32);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "non lowercase-hex char in {id}"
        );
    }

    #[test]
    fn read_cookie_finds_the_session_among_several() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            HeaderValue::from_static("theme=dark; me_demo_sid=abc123; other=1"),
        );
        assert_eq!(
            read_cookie(&headers, "me_demo_sid"),
            Some(SessionId("abc123".to_string()))
        );
    }

    #[test]
    fn read_cookie_rejects_a_prefix_collision_name() {
        let mut headers = HeaderMap::new();
        // A different cookie that merely starts with the session name must not match.
        headers.insert("cookie", HeaderValue::from_static("me_demo_sidX=abc123"));
        assert_eq!(read_cookie(&headers, "me_demo_sid"), None);
    }

    #[test]
    fn read_cookie_ignores_an_empty_value() {
        let mut headers = HeaderMap::new();
        headers.insert("cookie", HeaderValue::from_static("me_demo_sid="));
        assert_eq!(read_cookie(&headers, "me_demo_sid"), None);
    }
}
