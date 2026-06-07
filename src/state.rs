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
use crate::service::{FlaggedView, ViewMode};

/// Everything a request handler needs: the immutable config and settings, and
/// the scan cache. Shared as `Arc<AppState>`.
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) settings: Arc<ScanSettings>,
    pub(crate) cache: Cache,
}

/// The two scan slots and the staleness window, behind one mutex so a marker
/// write and a TTL rescan cannot interleave (see ADR-0002). A `None` TTL disables
/// caching (every read rescans).
pub(crate) struct Cache {
    entries: Mutex<ModeSlots>,
    ttl: Option<Duration>,
}

/// One cache entry per view mode. A slot is `None` until a viewer first asks for
/// that mode; the `all` slot stays cold, and pays no full walk, until then.
struct ModeSlots {
    gaps_only: Option<CacheEntry>,
    all: Option<CacheEntry>,
}

impl ModeSlots {
    fn slot(&mut self, mode: ViewMode) -> &mut Option<CacheEntry> {
        match mode {
            ViewMode::GapsOnly => &mut self.gaps_only,
            ViewMode::All => &mut self.all,
        }
    }
}

impl Cache {
    /// Return the cached view for `mode` if it is still fresh, otherwise build one
    /// under the lock and store it. Single-flight: the lock is held across `build`.
    pub(crate) async fn get_or_build<F, Fut>(&self, mode: ViewMode, build: F) -> Arc<FlaggedView>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = FlaggedView>,
    {
        let mut slots = self.entries.lock().await;
        if let Some(entry) = slots.slot(mode).as_ref()
            && let Some(ttl) = self.ttl
            && entry.stored_at.elapsed() < ttl
        {
            return Arc::clone(&entry.view);
        }
        let view = Arc::new(build().await);
        *slots.slot(mode) = Some(CacheEntry {
            stored_at: Instant::now(),
            view: Arc::clone(&view),
        });
        view
    }

    /// Build a fresh view for `mode` under the lock and store it, ignoring the TTL.
    pub(crate) async fn rebuild<F, Fut>(&self, mode: ViewMode, build: F) -> Arc<FlaggedView>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = FlaggedView>,
    {
        let mut slots = self.entries.lock().await;
        let view = Arc::new(build().await);
        *slots.slot(mode) = Some(CacheEntry {
            stored_at: Instant::now(),
            view: Arc::clone(&view),
        });
        view
    }

    /// Apply a marker write to both slots under one lock, then return the view for
    /// the requesting `mode`. Each warm slot is edited in place by its own closure
    /// (`edit_gaps` removes the marked subtree, `edit_all` covers it), leaving
    /// `stored_at` untouched so the TTL still fires on schedule (ADR-0002). A cold
    /// slot is left cold, except the requested one: when it is cold there is
    /// nothing to return, so it is built fresh (which already reflects the marker
    /// on disk).
    pub(crate) async fn edit_both_or_build<EG, EA, F, Fut>(
        &self,
        mode: ViewMode,
        edit_gaps: EG,
        edit_all: EA,
        build: F,
    ) -> Arc<FlaggedView>
    where
        EG: FnOnce(&mut FlaggedView),
        EA: FnOnce(&mut FlaggedView),
        F: FnOnce() -> Fut,
        Fut: Future<Output = FlaggedView>,
    {
        let mut slots = self.entries.lock().await;
        if let Some(entry) = slots.gaps_only.as_mut() {
            let mut view = (*entry.view).clone();
            edit_gaps(&mut view);
            entry.view = Arc::new(view);
        }
        if let Some(entry) = slots.all.as_mut() {
            let mut view = (*entry.view).clone();
            edit_all(&mut view);
            entry.view = Arc::new(view);
        }
        // Return the requested slot. If it was cold there is nothing to return, so
        // build it fresh. The `if let ... return` form (not a `match`) keeps the
        // read borrow from overlapping the later write, the same shape as
        // `get_or_build`.
        if let Some(entry) = slots.slot(mode).as_ref() {
            return Arc::clone(&entry.view);
        }
        let view = Arc::new(build().await);
        *slots.slot(mode) = Some(CacheEntry {
            stored_at: Instant::now(),
            view: Arc::clone(&view),
        });
        view
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
                entries: Mutex::new(ModeSlots {
                    gaps_only: None,
                    all: None,
                }),
                ttl,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ViewMode;
    use crate::service::{RootSection, RootState};

    fn sample_view(tag: &str) -> FlaggedView {
        vec![RootSection {
            path: tag.to_string(),
            state: RootState::Clean,
        }]
    }

    fn test_cache(ttl: Option<Duration>) -> Cache {
        Cache {
            entries: Mutex::new(ModeSlots {
                gaps_only: None,
                all: None,
            }),
            ttl,
        }
    }

    #[tokio::test]
    async fn get_or_build_returns_the_stored_view_within_ttl() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        let first = cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("first") })
            .await;
        let second = cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("second") })
            .await;
        assert!(
            Arc::ptr_eq(&first, &second),
            "a fresh cache must not rebuild"
        );
        assert_eq!(second[0].path, "first");
    }

    #[tokio::test]
    async fn the_two_slots_are_independent() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("gaps") })
            .await;
        // The all slot is cold, so it builds its own value, not the gaps one.
        let all = cache
            .get_or_build(ViewMode::All, || async { sample_view("all") })
            .await;
        assert_eq!(all[0].path, "all");
    }

    #[tokio::test]
    async fn rebuild_always_builds_and_stores_for_its_mode() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        let first = cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("first") })
            .await;
        let second = cache
            .rebuild(ViewMode::GapsOnly, || async { sample_view("second") })
            .await;
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second[0].path, "second");
    }

    #[tokio::test]
    async fn edit_both_or_build_edits_each_warm_slot() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("gaps") })
            .await;
        cache
            .get_or_build(ViewMode::All, || async { sample_view("all") })
            .await;
        let returned = cache
            .edit_both_or_build(
                ViewMode::All,
                |view| view[0].path = format!("{}-g", view[0].path),
                |view| view[0].path = format!("{}-a", view[0].path),
                || async { sample_view("rebuilt") },
            )
            .await;
        // The returned view is the all slot, edited by the all closure.
        assert_eq!(returned[0].path, "all-a");
        // The gaps slot was edited too, under the same lock.
        let gaps = cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("ignored") })
            .await;
        assert_eq!(gaps[0].path, "gaps-g");
    }

    #[tokio::test]
    async fn edit_both_or_build_builds_a_cold_requested_slot_and_leaves_the_other_cold() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        // Only the gaps slot is warm; request the all slot.
        cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("gaps") })
            .await;
        let returned = cache
            .edit_both_or_build(
                ViewMode::All,
                |view| view[0].path = format!("{}-g", view[0].path),
                |view| view[0].path = format!("{}-a", view[0].path),
                || async { sample_view("all-built") },
            )
            .await;
        // The requested (all) slot was cold, so it built fresh; its edit did not run.
        assert_eq!(returned[0].path, "all-built");
        // The warm gaps slot was edited in place.
        let gaps = cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("ignored") })
            .await;
        assert_eq!(gaps[0].path, "gaps-g");
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
