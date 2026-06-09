//! Drives the real proxy handler against a stub upstream, so the lifecycle is
//! exercised without launching the explore binary. A FakeLauncher reports a
//! sandbox already serving on the stub's port.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use missing_ebooks_demo_router::app;
use missing_ebooks_demo_router::config::Config;
use missing_ebooks_demo_router::ports::PortPool;
use missing_ebooks_demo_router::proxy::{AppState, Inner};
use missing_ebooks_demo_router::sandbox::{Launcher, Spawned};
use missing_ebooks_demo_router::session::SessionStore;
use tower::ServiceExt;

/// A stub that answers like the app: a full HTML page on `/`, with a body tag so
/// the banner has somewhere to land.
async fn start_stub() -> u16 {
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(|| async {
            ([("content-type", "text/html")], "<html><body>stub tree</body></html>")
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// A stub whose `/` answers with a 303 redirect to `/after`, mirroring the app's
/// Post/Redirect/Get on `/rescan`. A proxy that follows redirects surfaces the
/// 200 from `/after`; one that passes the redirect through surfaces the 303.
async fn start_redirecting_stub() -> u16 {
    let app = axum::Router::new()
        .route(
            "/",
            axum::routing::get(|| async { axum::response::Redirect::to("/after") }),
        )
        .route("/after", axum::routing::get(|| async { "landed" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// A stub that answers `/` with a plain-text body of `body_len` bytes, for
/// exercising the response-size cap.
async fn start_sized_stub(body_len: usize) -> u16 {
    let body = "x".repeat(body_len);
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(move || {
            let body = body.clone();
            async move { body }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// A stub that accepts connections and then holds them open without ever
/// answering, standing in for a sandbox that wedged after binding its socket.
/// Connecting succeeds; reading a response stalls until a timeout fires.
async fn start_silent_stub() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    port
}

/// A launcher that does not run `explore`: it assumes a stub is already listening
/// on the pooled port and stands in a throwaway `sleep` child so the router has a
/// real pid and handle to manage. `launches` counts calls so a test can assert a
/// reused session never re-spawns.
struct FakeLauncher {
    port: u16,
    launches: Arc<AtomicUsize>,
}

impl FakeLauncher {
    fn new(port: u16) -> Self {
        Self {
            port,
            launches: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl Launcher for FakeLauncher {
    async fn launch(&self, _scenario: &str, port: u16, _t: Duration) -> anyhow::Result<Spawned> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        assert_eq!(port, self.port, "router should spawn on the pooled port");
        // kill_on_drop keeps the stand-in child from outliving the test.
        let child = tokio::process::Command::new("sleep")
            .arg("3600")
            .kill_on_drop(true)
            .spawn()?;
        let pid = child.id().unwrap();
        Ok(Spawned { child, pid })
    }
}

/// A launcher that parks each launch on a release gate, so a test can hold
/// several spawns in flight at once and observe how the caps behave under
/// concurrency. `entered` is bumped once per launch the moment it parks;
/// `release` lets parked launches finish.
struct GatedLauncher {
    launches: Arc<AtomicUsize>,
    entered: Arc<tokio::sync::Semaphore>,
    release: Arc<tokio::sync::Semaphore>,
}

#[async_trait]
impl Launcher for GatedLauncher {
    async fn launch(&self, _scenario: &str, _port: u16, _t: Duration) -> anyhow::Result<Spawned> {
        let child = tokio::process::Command::new("sleep")
            .arg("3600")
            .kill_on_drop(true)
            .spawn()?;
        let pid = child.id().unwrap();
        self.launches.fetch_add(1, Ordering::SeqCst);
        // The reservation is already recorded (it happens before launch is
        // called), so announce this launch and park until the test releases it.
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(Spawned { child, pid })
    }
}

/// Drive one first-contact request from `ip` through the app and return its
/// status. With no cookie, this is the spawn path.
async fn fire(state: Arc<AppState>, ip: &'static str) -> StatusCode {
    app(state)
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", ip)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Assemble shared state the way `main` does, including the router's real HTTP
/// client, so the tests exercise the same request path the binary serves.
fn build_state(
    launcher: Box<dyn Launcher>,
    store: SessionStore,
    pool: PortPool,
    config: Config,
) -> Arc<AppState> {
    Arc::new(AppState {
        launcher,
        client: missing_ebooks_demo_router::proxy::http_client(),
        inner: tokio::sync::Mutex::new(Inner {
            store,
            pool,
            children: Default::default(),
        }),
        config,
    })
}

fn test_config(port_low: u16, port_high: u16) -> Config {
    Config {
        bind: "127.0.0.1:0".into(),
        port_low,
        port_high,
        max_sandboxes: 50,
        max_per_ip: 2,
        idle: Duration::from_secs(1200),
        ready_timeout: Duration::from_secs(5),
        forward_timeout: Duration::from_secs(30),
        max_response_bytes: 16 * 1024 * 1024,
        scenario: "mixed-forest".into(),
        explore_bin: "/unused".into(),
        cookie_name: "me_demo_sid".into(),
    }
}

#[tokio::test]
async fn first_request_spawns_sets_cookie_and_injects_banner() {
    let stub_port = start_stub().await;
    let config = test_config(stub_port, stub_port);
    let state = build_state(
        Box::new(FakeLauncher::new(stub_port)),
        SessionStore::new(config.max_sandboxes, config.max_per_ip),
        PortPool::new(config.port_low, config.port_high),
        config,
    );

    let response = app(state)
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", "203.0.113.7")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .expect("a new session sets a cookie")
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.contains("me_demo_sid="));
    assert!(set_cookie.contains("HttpOnly"));
    // The demo is served over HTTPS at the edge, so the cookie is Secure.
    assert!(set_cookie.contains("Secure"));

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("me-demo-banner"), "banner should be injected");
    assert!(html.contains("stub tree"));
}

#[tokio::test]
async fn global_capacity_returns_503_capacity_page() {
    let stub_port = start_stub().await;
    // A pool and cap of one, already filled, so the next visitor is refused.
    let mut config = test_config(stub_port, stub_port);
    config.max_sandboxes = 1;
    let mut store = SessionStore::new(1, 5);
    store.insert(
        missing_ebooks_demo_router::session::SessionId("taken".into()),
        missing_ebooks_demo_router::session::Sandbox {
            port: stub_port,
            pid: 0,
            client_ip: "9.9.9.9".into(),
            last_seen: std::time::Instant::now(),
        },
    );
    let mut pool = PortPool::new(stub_port, stub_port);
    pool.allocate();
    let state = build_state(Box::new(FakeLauncher::new(stub_port)), store, pool, config);

    let response = app(state)
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", "203.0.113.8")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("at capacity"));
}

#[tokio::test]
async fn upstream_redirects_are_passed_through_not_followed() {
    let stub_port = start_redirecting_stub().await;
    let config = test_config(stub_port, stub_port);
    let state = build_state(
        Box::new(FakeLauncher::new(stub_port)),
        SessionStore::new(config.max_sandboxes, config.max_per_ip),
        PortPool::new(config.port_low, config.port_high),
        config,
    );

    let response = app(state)
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", "203.0.113.9")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // The app uses Post/Redirect/Get (/rescan returns a 303), so the router must
    // hand the redirect to the browser rather than resolving it upstream.
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get("location")
            .expect("redirect Location is preserved")
            .to_str()
            .unwrap(),
        "/after"
    );
}

#[tokio::test]
async fn forward_failure_evicts_the_session() {
    // A port nothing listens on: bind to claim one, then drop the listener so the
    // proxy's forward gets connection-refused.
    let dead_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let config = test_config(dead_port, dead_port);
    let state = build_state(
        Box::new(FakeLauncher::new(dead_port)),
        SessionStore::new(config.max_sandboxes, config.max_per_ip),
        PortPool::new(config.port_low, config.port_high),
        config,
    );

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", "203.0.113.10")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    // The dead sandbox must not linger: the visitor's next request has to be able
    // to spawn a fresh one, so the session, its port, and its child are released.
    let inner = state.inner.lock().await;
    assert_eq!(inner.store.live_count(), 0, "session evicted");
    assert_eq!(inner.pool.available(), 1, "port returned to the pool");
    assert!(inner.children.is_empty(), "child handle dropped");
}

#[tokio::test]
async fn second_request_reuses_the_sandbox_without_respawning() {
    let stub_port = start_stub().await;
    let config = test_config(stub_port, stub_port);
    let launcher = FakeLauncher::new(stub_port);
    let launches = launcher.launches.clone();
    let state = build_state(
        Box::new(launcher),
        SessionStore::new(config.max_sandboxes, config.max_per_ip),
        PortPool::new(config.port_low, config.port_high),
        config,
    );

    // First request: no cookie, so the router spawns and sets the session cookie.
    let first = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", "203.0.113.11")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let set_cookie = first
        .headers()
        .get("set-cookie")
        .expect("first request sets a cookie")
        .to_str()
        .unwrap()
        .to_string();
    // The "me_demo_sid=<value>" pair, dropped of its attributes, to send back.
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();

    // Second request carries the cookie: the router reuses the same sandbox and
    // does not mint a new one.
    let second = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", "203.0.113.11")
                .header("cookie", &cookie_pair)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert!(
        second.headers().get("set-cookie").is_none(),
        "a reused session does not re-set the cookie"
    );
    assert_eq!(
        launches.load(Ordering::SeqCst),
        1,
        "the sandbox is spawned once and reused"
    );
}

#[tokio::test]
async fn oversized_sandbox_response_is_refused_without_eviction() {
    // The stub answers with a kilobyte; the cap is set well below that.
    let stub_port = start_sized_stub(1024).await;
    let mut config = test_config(stub_port, stub_port);
    config.max_response_bytes = 16;
    let state = build_state(
        Box::new(FakeLauncher::new(stub_port)),
        SessionStore::new(config.max_sandboxes, config.max_per_ip),
        PortPool::new(config.port_low, config.port_high),
        config,
    );

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", "203.0.113.14")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    // A chatty sandbox is still a healthy one, so the session is kept rather
    // than evicted the way an unreachable sandbox would be.
    let inner = state.inner.lock().await;
    assert_eq!(
        inner.store.live_count(),
        1,
        "an oversized response does not evict the session"
    );
}

#[tokio::test]
async fn forward_times_out_when_the_sandbox_never_responds() {
    let silent_port = start_silent_stub().await;
    let mut config = test_config(silent_port, silent_port);
    config.forward_timeout = Duration::from_secs(1);
    let state = build_state(
        Box::new(FakeLauncher::new(silent_port)),
        SessionStore::new(config.max_sandboxes, config.max_per_ip),
        PortPool::new(config.port_low, config.port_high),
        config,
    );

    // A sandbox that accepts the connection but never answers must not hang the
    // request: the forward timeout turns it into a 502. The outer bound catches
    // a regression where no timeout is set and the request hangs.
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        app(state.clone()).oneshot(
            Request::builder()
                .uri("/")
                .header("cf-connecting-ip", "203.0.113.13")
                .body(axum::body::Body::empty())
                .unwrap(),
        ),
    )
    .await
    .expect("forward must time out, not hang")
    .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    // A timed-out forward is a forward failure, so the dead session is evicted.
    let inner = state.inner.lock().await;
    assert_eq!(inner.store.live_count(), 0, "wedged session evicted");
}

#[tokio::test]
async fn wait_ready_gives_up_when_the_sandbox_never_answers() {
    use missing_ebooks_demo_router::sandbox::wait_ready;

    let silent_port = start_silent_stub().await;
    let client = missing_ebooks_demo_router::proxy::http_client();

    // The poll loop must honor its deadline even when a connected sandbox never
    // sends a response; otherwise a single send() blocks past the ready window.
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        wait_ready(&client, silent_port, Duration::from_secs(1)),
    )
    .await
    .expect("wait_ready must honor its deadline, not hang");
    assert!(result.is_err(), "a silent sandbox never becomes ready");
}

#[tokio::test]
async fn concurrent_first_contacts_from_one_ip_respect_the_per_ip_cap() {
    // Plenty of ports and global headroom, so only the per-IP cap (2) can bite.
    let config = test_config(9000, 9100);
    let launches = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let state = build_state(
        Box::new(GatedLauncher {
            launches: launches.clone(),
            entered: entered.clone(),
            release: release.clone(),
        }),
        SessionStore::new(config.max_sandboxes, config.max_per_ip),
        PortPool::new(config.port_low, config.port_high),
        config,
    );

    // Two concurrent first-contact requests from one IP. Each reserves a slot
    // and then parks inside launch, holding its reservation.
    let ip = "203.0.113.20";
    let r1 = tokio::spawn(fire(state.clone(), ip));
    let r2 = tokio::spawn(fire(state.clone(), ip));

    // Wait until both launches are in flight: both reservations are now held.
    let _two = entered.acquire_many(2).await.unwrap();

    // A third request from the same IP, while those two spawns are still in
    // flight, must be refused at once rather than spawning a third sandbox.
    // Before in-flight slots counted, all three passed the check and the third
    // would park inside its own launch, so a timeout here means the bug is back.
    let third = tokio::time::timeout(Duration::from_secs(2), fire(state.clone(), ip))
        .await
        .expect("third request must be refused immediately, not parked in a spawn");
    assert_eq!(
        third,
        StatusCode::SERVICE_UNAVAILABLE,
        "the per-IP cap must count in-flight spawns, not just committed ones"
    );
    assert_eq!(
        launches.load(Ordering::SeqCst),
        2,
        "only two sandboxes were launched"
    );

    // Let the parked launches finish so the test does not leak tasks.
    release.add_permits(2);
    let _ = r1.await;
    let _ = r2.await;
}

#[tokio::test]
async fn reaper_releases_idle_sandboxes() {
    use missing_ebooks_demo_router::proxy::reap_once;
    use missing_ebooks_demo_router::session::{Sandbox, SessionId};
    use std::time::Instant;

    let config = test_config(9000, 9000);
    let mut store = SessionStore::new(config.max_sandboxes, config.max_per_ip);
    let mut pool = PortPool::new(config.port_low, config.port_high);
    let port = pool.allocate().unwrap();

    // A stand-in child so the reaper has a real pid and handle to clean up.
    let child = tokio::process::Command::new("sleep")
        .arg("3600")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();

    let stale = Instant::now() - Duration::from_secs(3600);
    store.insert(
        SessionId("stale".into()),
        Sandbox {
            port,
            pid,
            client_ip: "203.0.113.12".into(),
            last_seen: stale,
        },
    );

    let state = build_state(Box::new(FakeLauncher::new(port)), store, pool, config);
    state.inner.lock().await.children.insert(pid, child);

    reap_once(&state, Instant::now(), Duration::from_secs(1200)).await;

    let inner = state.inner.lock().await;
    assert_eq!(inner.store.live_count(), 0, "idle sandbox reaped");
    assert_eq!(inner.pool.available(), 1, "port returned to the pool");
    assert!(inner.children.is_empty(), "child handle removed");
}
