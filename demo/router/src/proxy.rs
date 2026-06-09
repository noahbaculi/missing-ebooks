//! The axum fallback handler. One handler serves every path: it maps the request
//! to a sandbox (spawning one on first contact), reverse-proxies to it, and
//! injects the demo banner into full-page HTML responses.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tokio::sync::Mutex;

use crate::banner;
use crate::capacity::CAPACITY_HTML;
use crate::config::Config;
use crate::ports::PortPool;
use crate::sandbox::{self, Launcher};
use crate::session::{AdmitError, Sandbox, SessionId, SessionStore};

/// Everything the handler shares across requests.
pub struct AppState {
    pub config: Config,
    pub launcher: Box<dyn Launcher>,
    /// One HTTP client for every proxied request, built once and cloned from a
    /// shared inner Arc, so its connection pool is reused instead of rebuilt per
    /// request.
    pub client: reqwest::Client,
    /// Store and pool move together under one lock: every allocate pairs with an
    /// insert, and every reap pairs with a release, so a single mutex keeps them
    /// consistent without a second lock to order.
    pub inner: Mutex<Inner>,
}

/// Build the HTTP client the router uses for proxied requests and readiness
/// polls. One instance is built at startup and cloned (a `reqwest::Client` is an
/// Arc internally), so the connection pool is shared.
///
/// Redirects are not followed. The app relies on Post/Redirect/Get: `/rescan`
/// answers a full-page POST with a 303 to `/` (src/web.rs). That redirect has to
/// reach the visitor's browser so the address bar updates and a refresh does not
/// re-POST. Following it here would collapse the 303 into the index's 200 and
/// defeat the pattern.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // A connect that hangs (a port that accepts SYN but never completes the
        // handshake) must not stall a request. Loopback connects are instant, so
        // this ceiling only ever trips on a wedged sandbox. Per-request response
        // timeouts are set by the callers (forward, wait_ready).
        .connect_timeout(Duration::from_secs(2))
        .build()
        .expect("build reqwest client")
}

pub struct Inner {
    pub store: SessionStore,
    pub pool: PortPool,
    /// Live child handles, keyed by pid. Kept out of `Sandbox` (which stays
    /// `Clone` for the reaper) and held here so the reaper can `wait()` each
    /// child after SIGINT, reaping the zombie instead of leaking it under the
    /// container's PID 1.
    pub children: std::collections::HashMap<u32, tokio::process::Child>,
}

/// Headers that must not be copied verbatim between hops.
fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

/// The client IP, taken from Cloudflare's `CF-Connecting-IP` header. Behind the
/// tunnel the socket peer is always cloudflared, so this header is the only
/// truthful source; it is trusted because the tunnel is the sole ingress.
///
/// If the header is ever absent (only a tunnel misconfiguration, since Cloudflare
/// always sets it), every visitor collapses onto a single "unknown" IP and shares
/// the per-IP cap. That throttles the demo rather than failing open, which is the
/// safer direction.
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
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
/// expiring with the idle window. `Secure` is set because the demo is reached only
/// over HTTPS at the Cloudflare edge.
fn cookie_header(cookie_name: &str, sid: &SessionId, max_age_secs: u64) -> HeaderValue {
    let value = format!(
        "{cookie_name}={}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age_secs}",
        sid.0
    );
    HeaderValue::from_str(&value).expect("ascii cookie")
}

/// Liveness probe for the container healthcheck. It answers without touching
/// the session machinery, so probes neither spawn sandboxes nor count against
/// the per-IP cap the way a request to `/` would.
pub async fn health() -> StatusCode {
    StatusCode::OK
}

/// The handler. `axum` hands us the method, uri, headers, and body of every
/// request; we resolve the sandbox and forward.
pub async fn handle(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let now = Instant::now();
    let ip = client_ip(&headers);
    let existing = read_cookie(&headers, &state.config.cookie_name);

    // Resolve the serving port, the session in use, and (for a freshly created
    // sandbox) the cookie to set on the way out.
    let (port, sid, set_cookie) = match resolve_sandbox(&state, existing, &ip, now).await {
        Ok(resolved) => resolved,
        Err(Admit::Capacity) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [("content-type", "text/html; charset=utf-8")],
                CAPACITY_HTML,
            )
                .into_response();
        }
        Err(Admit::Spawn(err)) => {
            tracing::error!(%err, "sandbox spawn failed");
            return (StatusCode::BAD_GATEWAY, "demo backend unavailable").into_response();
        }
    };

    match forward(&state, port, method, &uri, &headers, body).await {
        Ok(mut response) => {
            if let Some(cookie) = set_cookie {
                // append, not insert: a sandbox could set its own cookie, and the
                // session cookie has to ride alongside it rather than replace it.
                response.headers_mut().append("set-cookie", cookie);
            }
            response
        }
        Err(err) => {
            tracing::error!(%err, port, "proxy forward failed");
            // The sandbox passed its readiness check but is unreachable now, so
            // drop the session: otherwise the cookie keeps routing this visitor to
            // the dead port until the idle reaper clears it. Evicting lets their
            // next request spawn a fresh sandbox.
            evict(&state, &sid).await;
            (StatusCode::BAD_GATEWAY, "demo backend unavailable").into_response()
        }
    }
}

/// Why resolution failed in a way the handler turns into a response.
enum Admit {
    Capacity,
    Spawn(anyhow::Error),
}

/// Find the sandbox for this session, or spawn one. Returns the serving port, the
/// session id in use, and (for a freshly spawned sandbox) the cookie to set.
async fn resolve_sandbox(
    state: &Arc<AppState>,
    existing: Option<SessionId>,
    ip: &str,
    now: Instant,
) -> Result<(u16, SessionId, Option<HeaderValue>), Admit> {
    // Fast path: a known, live session. Touch it and reuse its port.
    if let Some(sid) = existing {
        let mut inner = state.inner.lock().await;
        if let Some(port) = inner.store.touch(&sid, now) {
            return Ok((port, sid, None));
        }
    }

    // Slow path: reserve an in-flight slot under the caps and take a port while
    // holding the lock, then spawn outside the lock so a slow seed does not block
    // other requests. Reserving (not just checking) under the lock is what stops
    // a burst of first-contact requests from one IP all passing the cap and then
    // over-spawning while their launches overlap.
    let port = {
        let mut inner = state.inner.lock().await;
        match inner.store.reserve(ip) {
            Ok(()) => {}
            Err(AdmitError::AtCapacity) | Err(AdmitError::PerIpLimit) => {
                return Err(Admit::Capacity);
            }
        }
        match inner.pool.allocate() {
            Some(port) => port,
            None => {
                // The cap had room but the port pool did not: hand the
                // reservation back so it does not pin the slot forever.
                inner.store.release_reservation(ip);
                return Err(Admit::Capacity);
            }
        }
    };

    let spawned = match state
        .launcher
        .launch(&state.config.scenario, port, state.config.ready_timeout)
        .await
    {
        Ok(spawned) => spawned,
        Err(err) => {
            // Hand the port and the reservation back so a failed spawn leaks
            // neither the port nor the in-flight cap slot.
            let mut inner = state.inner.lock().await;
            inner.pool.release(port);
            inner.store.release_reservation(ip);
            return Err(Admit::Spawn(err));
        }
    };

    let sid = new_session_id();
    let cookie = cookie_header(
        &state.config.cookie_name,
        &sid,
        state.config.idle.as_secs(),
    );
    let mut inner = state.inner.lock().await;
    // Park the child handle by pid so the reaper can wait() on it after SIGINT.
    inner.children.insert(spawned.pid, spawned.child);
    // commit drops the reservation and records the live sandbox under one lock.
    inner.store.commit(
        ip,
        sid.clone(),
        Sandbox {
            port,
            pid: spawned.pid,
            client_ip: ip.to_string(),
            last_seen: now,
        },
    );
    Ok((port, sid, Some(cookie)))
}

/// Drop a session whose sandbox is unreachable: remove its row, return its port to
/// the pool, and SIGINT the process so `explore` clears its temp dir (the startup
/// sweep is the backstop if it is already gone). The child is waited off the
/// request path so it does not linger as a zombie under the container's PID 1.
async fn evict(state: &Arc<AppState>, sid: &SessionId) {
    let child = {
        let mut inner = state.inner.lock().await;
        let Some(sandbox) = inner.store.remove(sid) else {
            return;
        };
        inner.pool.release(sandbox.port);
        sandbox::shutdown(sandbox.pid);
        inner.children.remove(&sandbox.pid)
    };
    if let Some(mut child) = child {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

/// One reaper pass: remove every sandbox idle past `idle` as of `now`, return its
/// port to the pool, SIGINT its process so `explore` clears its temp dir, and wait
/// the child off the calling task so it does not linger as a zombie under PID 1.
/// The binary's reaper loop calls this on a timer; tests call it directly.
pub async fn reap_once(state: &Arc<AppState>, now: Instant, idle: Duration) {
    let (reaped, mut children) = {
        let mut inner = state.inner.lock().await;
        let reaped = inner.store.reap_idle(now, idle);
        let mut children = Vec::new();
        for s in &reaped {
            inner.pool.release(s.port);
            if let Some(child) = inner.children.remove(&s.pid) {
                children.push(child);
            }
        }
        (reaped, children)
    };
    for s in &reaped {
        // SIGINT lets explore remove its temp dir before exiting.
        sandbox::shutdown(s.pid);
        tracing::info!(port = s.port, pid = s.pid, "reaped idle sandbox");
    }
    for mut child in children.drain(..) {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

/// Forward one request to the sandbox on `port` and return its response, with the
/// banner injected when the response is a full HTML page (not an htmx partial).
async fn forward(
    state: &Arc<AppState>,
    port: u16,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Bytes,
) -> anyhow::Result<Response> {
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let url = format!("http://127.0.0.1:{port}{path}");

    let mut req = state
        .client
        .request(method.clone(), &url)
        .timeout(state.config.forward_timeout);
    for (name, value) in headers.iter() {
        if !is_hop_by_hop(name.as_str()) {
            req = req.header(name.as_str(), value.as_bytes());
        }
    }
    let mut upstream = req.body(body.to_vec()).send().await?;

    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let is_html = upstream_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/html"))
        .unwrap_or(false);
    let is_htmx = headers.contains_key("hx-request");
    // A content-encoding (gzip and the like) means the bytes are not the literal
    // HTML, so splicing the banner in would corrupt the body and leave the
    // recomputed length wrong. The app serves uncompressed pages today, but
    // honoring the header keeps the splice safe if that ever changes.
    let is_encoded = upstream_headers.contains_key("content-encoding");

    // Buffer the body with a ceiling. The banner splice needs the whole HTML page
    // in hand, so streaming straight through is not an option for full pages;
    // capping the accumulation keeps a runaway response from ballooning router
    // memory. An oversized response is the sandbox misbehaving, not dying, so it
    // becomes a 502 here and the caller leaves the session in place.
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = upstream.chunk().await? {
        if bytes.len() + chunk.len() > state.config.max_response_bytes {
            tracing::warn!(port, cap = state.config.max_response_bytes, "sandbox response exceeded the body cap");
            return Ok((StatusCode::BAD_GATEWAY, "demo backend response too large").into_response());
        }
        bytes.extend_from_slice(&chunk);
    }

    let mut builder = Response::builder().status(status);
    let final_body: Vec<u8> = if is_html && !is_htmx && !is_encoded {
        banner::inject(&String::from_utf8_lossy(&bytes)).into_bytes()
    } else {
        bytes
    };

    // Copy upstream headers except hop-by-hop and content-length, which we reset
    // to the (possibly grown) injected length.
    for (name, value) in upstream_headers.iter() {
        let lname = name.as_str().to_ascii_lowercase();
        if is_hop_by_hop(&lname) || lname == "content-length" {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    builder = builder.header("content-length", final_body.len());
    Ok(builder
        .body(axum::body::Body::from(final_body))
        .expect("valid response")
        .into_response())
}
