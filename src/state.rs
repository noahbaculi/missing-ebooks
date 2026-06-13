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
use crate::service::{FlaggedView, RootSection, ViewMode};

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

/// Stamp and store a freshly built view in a slot. The single place that sets
/// `stored_at = now`: a fresh build refreshes the freshness clock (ADR-0002).
fn store_fresh(slot: &mut Option<CacheEntry>, view: FlaggedView) -> Arc<FlaggedView> {
    let view = Arc::new(view);
    *slot = Some(CacheEntry {
        stored_at: Instant::now(),
        view: Arc::clone(&view),
    });
    view
}

/// The write-path tail: return the requested mode's warm slot, or build it fresh
/// when it is cold. The `if let … return` form keeps the read borrow from
/// overlapping the later write.
async fn return_or_build<F, Fut>(
    slots: &mut ModeSlots,
    mode: ViewMode,
    build: F,
) -> Arc<FlaggedView>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = FlaggedView>,
{
    if let Some(entry) = slots.slot(mode).as_ref() {
        return Arc::clone(&entry.view);
    }
    let view = build().await;
    store_fresh(slots.slot(mode), view)
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
            tracing::debug!(mode = mode.as_query(), "cache hit");
            return Arc::clone(&entry.view);
        }
        tracing::debug!(mode = mode.as_query(), "cache miss");
        let view = build().await;
        store_fresh(slots.slot(mode), view)
    }

    /// Build a fresh view for `mode` under the lock and store it, ignoring the TTL.
    pub(crate) async fn rebuild<F, Fut>(&self, mode: ViewMode, build: F) -> Arc<FlaggedView>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = FlaggedView>,
    {
        let mut slots = self.entries.lock().await;
        let view = build().await;
        store_fresh(slots.slot(mode), view)
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
        return_or_build(&mut slots, mode, build).await
    }

    /// Rescan one root and replace its section in each warm slot, under one lock,
    /// leaving `stored_at` untouched so the TTL still fires on schedule. Used by
    /// undo: a marker delete can re-flag a subtree, and the cached view discarded
    /// that structure when it marked, so the section is rebuilt by a fresh per-root
    /// scan rather than edited in place. `rebuild_section` produces the section for
    /// a given mode; a cold requested slot is built fresh with `build_full`.
    pub(crate) async fn rebuild_root<RS, RsFut, B, BFut>(
        &self,
        root: usize,
        mode: ViewMode,
        mut rebuild_section: RS,
        build_full: B,
    ) -> Arc<FlaggedView>
    where
        RS: FnMut(ViewMode) -> RsFut,
        RsFut: Future<Output = RootSection>,
        B: FnOnce() -> BFut,
        BFut: Future<Output = FlaggedView>,
    {
        let mut slots = self.entries.lock().await;
        if slots.gaps_only.is_some() {
            let section = rebuild_section(ViewMode::GapsOnly).await;
            let entry = slots.gaps_only.as_mut().expect("checked is_some above");
            let mut view = (*entry.view).clone();
            if root < view.len() {
                view[root] = section;
            }
            entry.view = Arc::new(view);
        }
        if slots.all.is_some() {
            let section = rebuild_section(ViewMode::All).await;
            let entry = slots.all.as_mut().expect("checked is_some above");
            let mut view = (*entry.view).clone();
            if root < view.len() {
                view[root] = section;
            }
            entry.view = Arc::new(view);
        }
        return_or_build(&mut slots, mode, build_full).await
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

    #[tokio::test]
    async fn rebuild_root_replaces_one_root_in_each_warm_slot() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        // Two roots per slot so we can prove only index 1 is touched.
        let two = || {
            vec![
                RootSection {
                    path: "keep".to_string(),
                    state: RootState::Clean,
                },
                RootSection {
                    path: "old".to_string(),
                    state: RootState::Clean,
                },
            ]
        };
        cache
            .get_or_build(ViewMode::GapsOnly, || async { two() })
            .await;
        cache.get_or_build(ViewMode::All, || async { two() }).await;

        let returned = cache
            .rebuild_root(
                1,
                ViewMode::GapsOnly,
                |mode| async move {
                    RootSection {
                        path: format!("new-{}", mode.as_query()),
                        state: RootState::Clean,
                    }
                },
                || async { two() },
            )
            .await;

        // Index 0 untouched, index 1 rebuilt with the gaps-mode section.
        assert_eq!(returned[0].path, "keep");
        assert_eq!(returned[1].path, "new-gaps");
        // The all slot was rebuilt with the all-mode section under the same lock.
        let all = cache.get_or_build(ViewMode::All, || async { two() }).await;
        assert_eq!(all[1].path, "new-all");
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

    #[tokio::test]
    async fn fresh_build_stamps_stored_at_and_edit_leaves_it() {
        let cache = test_cache(Some(Duration::from_secs(60)));

        // A build stamps the clock.
        cache
            .get_or_build(ViewMode::GapsOnly, || async { sample_view("v1") })
            .await;
        let stamped = {
            let slots = cache.entries.lock().await;
            slots.gaps_only.as_ref().unwrap().stored_at
        };

        // An in-place edit changes the data but not the clock.
        cache
            .edit_both_or_build(
                ViewMode::GapsOnly,
                |view| view[0].path = "edited".to_string(),
                |_view| {},
                || async { sample_view("unused") },
            )
            .await;
        let after_edit = {
            let slots = cache.entries.lock().await;
            slots.gaps_only.as_ref().unwrap().stored_at
        };
        assert_eq!(stamped, after_edit, "an edit must not bump stored_at");
    }
}
