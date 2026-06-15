//! The shared state behind the demo handlers: the read-only base views (one per
//! view mode), the in-memory session table, the demo config, and the render
//! search links. The base views are scanned once at startup; each request clones
//! the one it needs and replays the session's marks on top.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{Config, SearchLink};
use crate::scanner::ScanSettings;
use crate::service::{FlaggedView, ViewMode, build_view};

use super::session::SessionStore;

/// Runtime knobs for the demo server, read from the environment by the binary.
pub struct DemoConfig {
    /// Address the demo HTTP server binds to, e.g. `0.0.0.0:8080`.
    pub bind: String,
    /// Seeded scenario name shown to every visitor.
    pub scenario: String,
    /// Hard ceiling on concurrent in-memory sessions.
    pub max_sessions: usize,
    /// How long a session may sit idle before the reaper drops it; also the
    /// cookie `Max-Age`.
    pub idle: Duration,
    /// Name of the session cookie.
    pub cookie_name: String,
}

/// Everything the demo handlers share. Held as `Arc<DemoState>`.
pub struct DemoState {
    pub(crate) base_gaps: Arc<FlaggedView>,
    pub(crate) base_all: Arc<FlaggedView>,
    pub(crate) sessions: Mutex<SessionStore>,
    pub(crate) config: DemoConfig,
    pub(crate) search_links: Vec<SearchLink>,
}

impl DemoState {
    /// The shared base view for a mode, built once at startup.
    pub(crate) fn base(&self, mode: ViewMode) -> &FlaggedView {
        match mode {
            ViewMode::GapsOnly => &self.base_gaps,
            ViewMode::All => &self.base_all,
        }
    }

    /// How many library roots the base views carry. Bounds the root index a mark
    /// may name.
    pub(crate) fn num_roots(&self) -> usize {
        self.base_gaps.len()
    }

    /// Drop every session idle past the configured window as of `now`; returns the
    /// number reaped. Called on a timer by the binary's reaper task.
    pub fn reap_idle(&self, now: Instant) -> usize {
        self.sessions
            .lock()
            .expect("session lock")
            .reap_idle(now, self.config.idle)
    }
}

/// Scan the seeded library into the two base views and assemble the demo state.
/// Runs once at startup.
pub async fn build_state(
    config: Config,
    settings: ScanSettings,
    demo_config: DemoConfig,
) -> DemoState {
    let settings = Arc::new(settings);
    // The demo scans the seeded library once into static base views and never
    // rescans, so it carries no index: pass None for a plain listing walk.
    let base_gaps = Arc::new(build_view(&config, &settings, ViewMode::GapsOnly, None).await);
    let base_all = Arc::new(build_view(&config, &settings, ViewMode::All, None).await);
    DemoState {
        base_gaps,
        base_all,
        sessions: Mutex::new(SessionStore::new(demo_config.max_sessions)),
        search_links: config.search_links,
        config: demo_config,
    }
}
