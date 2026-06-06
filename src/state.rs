//! Application state: the immutable `Arc<Config>` and compiled `Arc<ScanSettings>`,
//! plus a TTL-memoized scan cache behind one mutex. The cache is filled on the
//! first read and refreshed by the TTL or a rescan; a marker write updates the
//! stored view in place rather than rewalking (see docs/adr/0002-v1-runtime-write-model.md).

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::Config;
use crate::scanner::ScanSettings;
use crate::service::FlaggedView;

/// Everything a request handler needs: the immutable config and settings, and
/// the scan cache. Shared as `Arc<AppState>`.
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) settings: Arc<ScanSettings>,
    pub(crate) cache: Cache,
}

/// The scan cache: one slot guarded by a mutex, plus the staleness window. A
/// `None` TTL disables caching (every read rescans).
pub(crate) struct Cache {
    entry: Mutex<Option<CacheEntry>>,
    ttl: Option<Duration>,
}

impl Cache {
    /// Return the cached view if it is still fresh, otherwise build one under the
    /// lock and store it. Single-flight: the lock is held across `build`, so a
    /// concurrent stale reader blocks and then returns the view this call stored.
    pub(crate) async fn get_or_build<F, Fut>(&self, build: F) -> Arc<FlaggedView>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = FlaggedView>,
    {
        let mut guard = self.entry.lock().await;
        if let Some(entry) = guard.as_ref()
            && let Some(ttl) = self.ttl
            && entry.stored_at.elapsed() < ttl
        {
            return Arc::clone(&entry.view);
        }
        let view = Arc::new(build().await);
        *guard = Some(CacheEntry {
            stored_at: Instant::now(),
            view: Arc::clone(&view),
        });
        view
    }

    /// Build a fresh view under the lock and store it, ignoring the TTL. Shares
    /// the lock with `get_or_build`, so a rescan and a stale read cannot both scan.
    pub(crate) async fn rebuild<F, Fut>(&self, build: F) -> Arc<FlaggedView>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = FlaggedView>,
    {
        let mut guard = self.entry.lock().await;
        let view = Arc::new(build().await);
        *guard = Some(CacheEntry {
            stored_at: Instant::now(),
            view: Arc::clone(&view),
        });
        view
    }

    /// Edit the stored view in place under the lock and return it, leaving
    /// `stored_at` untouched so the TTL still fires on schedule (see ADR-0002).
    /// When the cache is cold there is nothing to edit, so build a fresh view and
    /// store it instead, stamping `stored_at`.
    pub(crate) async fn edit_or_build<E, F, Fut>(&self, edit: E, build: F) -> Arc<FlaggedView>
    where
        E: FnOnce(&mut FlaggedView),
        F: FnOnce() -> Fut,
        Fut: Future<Output = FlaggedView>,
    {
        let mut guard = self.entry.lock().await;
        match guard.as_mut() {
            Some(entry) => {
                let mut view = (*entry.view).clone();
                edit(&mut view);
                entry.view = Arc::new(view);
                Arc::clone(&entry.view)
            }
            None => {
                let view = Arc::new(build().await);
                *guard = Some(CacheEntry {
                    stored_at: Instant::now(),
                    view: Arc::clone(&view),
                });
                view
            }
        }
    }
}

/// A stored view and the instant it was scanned.
pub(crate) struct CacheEntry {
    pub(crate) stored_at: Instant,
    pub(crate) view: Arc<FlaggedView>,
}

impl AppState {
    /// Build the shared state. `ttl_seconds == 0` disables the cache; any other
    /// value sets the staleness window.
    pub fn new(config: Config, settings: ScanSettings) -> AppState {
        let ttl = if config.ttl_seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(config.ttl_seconds))
        };
        AppState {
            config: Arc::new(config),
            settings: Arc::new(settings),
            cache: Cache {
                entry: Mutex::new(None),
                ttl,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{RootSection, RootState};

    fn sample_view(tag: &str) -> FlaggedView {
        vec![RootSection {
            path: tag.to_string(),
            state: RootState::Clean,
        }]
    }

    #[tokio::test]
    async fn get_or_build_returns_the_stored_view_within_ttl() {
        let cache = Cache {
            entry: Mutex::new(None),
            ttl: Some(Duration::from_secs(600)),
        };
        let first = cache.get_or_build(|| async { sample_view("first") }).await;
        // A second call within the TTL must not build again.
        let second = cache.get_or_build(|| async { sample_view("second") }).await;
        assert!(Arc::ptr_eq(&first, &second), "a fresh cache must not rebuild");
        assert_eq!(second[0].path, "first");
    }

    #[tokio::test]
    async fn rebuild_always_builds_and_stores() {
        let cache = Cache {
            entry: Mutex::new(None),
            ttl: Some(Duration::from_secs(600)),
        };
        let first = cache.get_or_build(|| async { sample_view("first") }).await;
        let second = cache.rebuild(|| async { sample_view("second") }).await;
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second[0].path, "second");
    }

    #[tokio::test]
    async fn edit_or_build_edits_a_warm_cache_without_building() {
        let cache = Cache {
            entry: Mutex::new(None),
            ttl: Some(Duration::from_secs(600)),
        };
        cache.get_or_build(|| async { sample_view("warm") }).await;
        let edited = cache
            .edit_or_build(
                |view| view[0].path = "edited".to_string(),
                || async { sample_view("rebuilt") },
            )
            .await;
        // Warm cache: the edit ran and no rebuild happened, so the value derives
        // from "warm" (now "edited"), never "rebuilt".
        assert_eq!(edited[0].path, "edited");
    }

    #[tokio::test]
    async fn edit_or_build_builds_when_cold() {
        let cache = Cache {
            entry: Mutex::new(None),
            ttl: Some(Duration::from_secs(600)),
        };
        let view = cache
            .edit_or_build(
                |view| view[0].path = "must-not-run".to_string(),
                || async { sample_view("cold-build") },
            )
            .await;
        assert_eq!(view[0].path, "cold-build");
    }

    fn settings() -> ScanSettings {
        ScanSettings::compile(Config::default().scan_inputs()).unwrap()
    }

    #[test]
    fn ttl_zero_disables_the_cache() {
        let cfg = Config {
            ttl_seconds: 0,
            ..Default::default()
        };
        let state = AppState::new(cfg, settings());
        assert_eq!(state.cache.ttl, None);
    }

    #[test]
    fn nonzero_ttl_sets_the_window() {
        let cfg = Config {
            ttl_seconds: 90,
            ..Default::default()
        };
        let state = AppState::new(cfg, settings());
        assert_eq!(state.cache.ttl, Some(Duration::from_secs(90)));
    }
}
