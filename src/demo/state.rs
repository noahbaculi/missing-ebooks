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
    /// How long a session may sit idle before the reaper drops it. Also the
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

    /// Acquire the session store lock, recovering on poison.
    ///
    /// Poison means a previous thread panicked while holding the lock. The
    /// session table itself is intact as far as the surviving thread can
    /// tell, so we proceed with a `tracing::warn` rather than propagate the
    /// panic. The poison-recovery pattern duplicates `DirIndex`'s internal lock
    /// recovery and `autosync::lock_inner`.
    pub(crate) fn lock_sessions(&self) -> std::sync::MutexGuard<'_, SessionStore> {
        self.sessions.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("demo session mutex poisoned; recovering");
            poisoned.into_inner()
        })
    }

    /// Drop every session idle past the configured window as of `now`. Returns the
    /// number reaped. Called on a timer by the binary's reaper task.
    pub fn reap_idle(&self, now: Instant) -> usize {
        self.lock_sessions().reap_idle(now, self.config.idle)
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
    // The demo scans once into a static raw view and never rescans, so the
    // per-root indices fed in are throwaway: they populate as the walks go,
    // then drop with this local Vec.
    let throwaway_indices: Vec<_> = (0..config.library_roots.len())
        .map(|_| Arc::new(DirIndex::new()))
        .collect();
    let base_raw = Arc::new(build_view(&config, &settings, &throwaway_indices).await);
    DemoState {
        base_raw,
        sessions: Mutex::new(SessionStore::new(demo_config.max_sessions)),
        search_links: config.search_links,
        config: demo_config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw_view::RawView;

    /// Build a minimal DemoState whose base_raw is empty (no roots). Enough
    /// to exercise the session-store lock. No scan runs.
    fn test_state() -> DemoState {
        DemoState {
            base_raw: Arc::new(RawView::new()),
            sessions: Mutex::new(SessionStore::new(8)),
            config: DemoConfig {
                bind: "127.0.0.1:0".to_string(),
                scenario: "test".to_string(),
                max_sessions: 8,
                idle: Duration::from_secs(60),
                cookie_name: "me_demo_sid".to_string(),
            },
            search_links: Vec::new(),
        }
    }

    #[test]
    fn lock_sessions_recovers_from_poisoning() {
        let state = Arc::new(test_state());

        // Poison the mutex: take the guard on a worker thread, then panic.
        let poisoner = Arc::clone(&state);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.sessions.lock().unwrap();
            panic!("intentional poison for test");
        })
        .join();

        // The bare std API would now panic on .expect(...).
        assert!(
            state.sessions.lock().is_err(),
            "test setup failed: mutex was not poisoned"
        );

        // lock_sessions must recover and return a usable guard.
        let guard = state.lock_sessions();
        assert_eq!(guard.len(), 0, "recovered guard exposes the prior state");
        drop(guard);
    }
}
