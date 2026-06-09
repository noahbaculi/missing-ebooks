//! The demo's axum router and handlers. Handlers reuse the production `page`,
//! `render_section`, `apply_mark`, and `apply_mark_all` from the library, plus
//! the static-asset handlers. A visitor is pinned to an in-memory session by a
//! cookie; their marks are replayed on top of the shared base view per request.
//! The full index page carries the demo banner; the `/mark` partial does not.

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};

use crate::service::{FlaggedView, ViewMode, apply_mark, apply_mark_all};
use crate::web::{MarkRequest, ViewQuery, app_css, htmx_script, page, render_section};

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
        .route("/rescan", post(rescan))
        .route("/healthz", get(healthz))
        .route("/static/htmx.min.js", get(htmx_script))
        .route("/static/app.css", get(app_css))
        .with_state(state)
}

/// Pull the session id out of the `Cookie` header for `cookie_name`.
fn read_cookie(headers: &HeaderMap, cookie_name: &str) -> Option<SessionId> {
    let raw = headers.get("cookie")?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(&format!("{cookie_name}="))
            && !value.is_empty()
        {
            return Some(SessionId(value.to_string()));
        }
    }
    None
}

/// Mint a new random session id as 32 hex characters.
fn new_session_id() -> SessionId {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("OS rng");
    SessionId(buf.iter().map(|b| format!("{b:02x}")).collect())
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

/// Derive one session's view for a mode: clone the shared base and replay the
/// session's marks. A mark naming an out-of-range root is skipped defensively;
/// an unmatched `rel` is a no-op inside the overlay functions.
fn derive_view(base: &FlaggedView, marks: &[Mark], mode: ViewMode) -> FlaggedView {
    let mut view = base.to_vec();
    for mark in marks {
        let Some(section) = view.get_mut(mark.root) else {
            continue;
        };
        match mode {
            ViewMode::GapsOnly => apply_mark(section, &mark.rel),
            ViewMode::All => apply_mark_all(section, &mark.rel, mark.kind),
        }
    }
    view
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
    let view = derive_view(state.base(mode), &marks, mode);
    let html = page(&view, &state.search_links, mode).into_string();
    let mut response = Html(banner::inject(&html)).into_response();
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
    let view = derive_view(state.base(mode), &marks, mode);
    let markup = render_section(&view[req.root], req.root, None, &state.search_links, mode);
    let mut response = Html(markup.into_string()).into_response();
    if let Some(cookie) = set_cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

async fn rescan(Form(query): Form<ViewQuery>) -> Redirect {
    // Every GET re-derives, and marks live in the session, so a rescan just
    // returns the visitor to their current view. 303 See Other (Post/Redirect/Get).
    let mode = ViewMode::from_query(query.view.as_deref());
    Redirect::to(match mode {
        ViewMode::GapsOnly => "/",
        ViewMode::All => "/?view=all",
    })
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
}
