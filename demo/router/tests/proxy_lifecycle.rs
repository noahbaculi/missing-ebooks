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
