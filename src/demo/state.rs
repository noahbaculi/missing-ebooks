//! The shared state behind the demo handlers: the shared raw walked view
//! built once at startup, the in-memory session table, the demo config, and
//! the render search links. The raw view is scanned once at startup. Each
//! request clones it before replaying the session's marks and renders per
//! mode (see ADR-0022).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{Config, SearchLink};
use crate::raw_view::{RawView, build_view};
use crate::scanner::{DirIndex, ScanSettings};

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
    /// The raw walked view, shared and immutable across sessions. Each request
    /// clones it before replaying the session's marks and rendering per mode.
    pub(crate) base_raw: Arc<RawView>,
    pub(crate) sessions: Mutex<SessionStore>,
    pub(crate) config: DemoConfig,
    pub(crate) search_links: Vec<SearchLink>,
}

impl DemoState {
    /// How many library roots the base view carries. Bounds the root index a
    /// mark may name.
    pub(crate) fn num_roots(&self) -> usize {
        self.base_raw.len()
    }

    /// Drop every session idle past the configured window as of `now`. Returns the
    /// number reaped. Called on a timer by the binary's reaper task.
    pub fn reap_idle(&self, now: Instant) -> usize {
        self.sessions
            .lock()
            .expect("session lock")
            .reap_idle(now, self.config.idle)
    }
}

/// Scan the seeded library into the shared raw view and assemble the demo
/// state. Runs once at startup.
pub async fn build_state(
    config: Config,
    settings: ScanSettings,
    demo_config: DemoConfig,
) -> DemoState {
    let settings = Arc::new(settings);
    // The demo scans once into a static raw view and never rescans, so the dir
    // index this build feeds in is throwaway: it populates as the walk goes,
    // then drops with the local Arc.
    let throwaway_index = Arc::new(std::sync::Mutex::new(DirIndex::new()));
    let base_raw = Arc::new(build_view(&config, &settings, throwaway_index).await);
    DemoState {
        base_raw,
        sessions: Mutex::new(SessionStore::new(demo_config.max_sessions)),
        search_links: config.search_links,
        config: demo_config,
    }
}
