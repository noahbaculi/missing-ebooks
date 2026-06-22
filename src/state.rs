//! Application state: the immutable `Arc<Config>` and compiled `Arc<ScanSettings>`,
//! plus a TTL-memoized scan cache behind one mutex. The cache stores the raw
//! per-root walk output and the response renders per `ViewMode` on each read
//! (see ADR-0022); a marker write updates the stored raw view in place rather
//! than rewalking (see docs/adr/0002-v1-runtime-write-model.md).

use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::Config;
use crate::scanner::{DirIndex, ScanSettings, ScannedFolder};

/// Everything a request handler needs: the immutable config and settings, the
/// scan cache, and the autosync registry. Shared as `Arc<AppState>`.
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) settings: Arc<ScanSettings>,
    /// The shared per-directory mtime index. Read by every scan path and
    /// discarded only on a `/rescan` click or process restart (see ADR-0020,
    /// ADR-0023). A blocking scan locks it to reuse unchanged directories.
    pub(crate) dir_index: Arc<StdMutex<DirIndex>>,
    pub(crate) cache: Cache,
    /// The autosync subscriber registry and loop handle. The loop spawns on the
    /// first SSE subscription with a non-zero `autosync_interval_seconds` and
    /// exits when the last subscriber disconnects (ADR-0023).
    pub(crate) autosync: crate::autosync::Autosync,
}

/// The result of scanning one root, in raw form: enough to render either view
/// without re-walking.
#[derive(Debug, Clone)]
pub struct RawRootSection {
    /// The canonical root path when it resolved, else the configured path.
    pub path: String,
    /// What the scan found for this root.
    pub state: RawRootState,
}

/// Per-root state held by the cache. `Walked` carries the full
/// `Vec<ScannedFolder>` the scanner emitted; the response reduces it per mode.
/// `Clean` is stored when the walk produced no entries at all so a render
/// decision does not have to inspect an empty `Vec`. `Error` carries the
/// scanner's message for a root that could not be canonicalized or was not a
/// directory.
#[derive(Debug, Clone)]
pub enum RawRootState {
    /// The scan completed and produced no entries to render.
    Clean,
    /// The scan completed; render the gaps or show-all view from these folders.
    Walked(Vec<ScannedFolder>),
    /// The scan failed; the message is surfaced in the response.
    Error(String),
}

/// The whole raw view: one section per configured library root, in config order.
pub type RawView = Vec<RawRootSection>;

/// The cache: one raw slot and the staleness window, behind one mutex so a
/// marker write and a TTL rescan cannot interleave (see ADR-0002). A `None` TTL
/// disables caching (every read rescans).
pub(crate) struct Cache {
    entries: Mutex<Option<CacheEntry>>,
    ttl: Option<Duration>,
}

/// A stored raw view and the instant it was scanned.
pub(crate) struct CacheEntry {
    pub(crate) stored_at: Instant,
    pub(crate) raw: Arc<RawView>,
}

/// Stamp and store a freshly built raw view. The single place that sets
/// `stored_at = now`: a fresh build refreshes the freshness clock (ADR-0002).
/// `apply_marker_or_build` uses `Arc::make_mut` for in-place edits and
/// intentionally bypasses this function, leaving `stored_at` unchanged.
fn store_fresh(slot: &mut Option<CacheEntry>, raw: RawView) -> Arc<RawView> {
    let raw = Arc::new(raw);
    *slot = Some(CacheEntry {
        stored_at: Instant::now(),
        raw: Arc::clone(&raw),
    });
    raw
}

impl Cache {
    /// Return the cached raw view if it is still fresh, otherwise build one
    /// under the lock and store it. Single-flight: the lock is held across
    /// `build`.
    pub(crate) async fn get_or_build<F, Fut>(&self, build: F) -> Arc<RawView>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = RawView>,
    {
        let mut slot = self.entries.lock().await;
        if let Some(entry) = slot.as_ref()
            && is_fresh(entry, self.ttl)
        {
            tracing::debug!("cache hit");
            return Arc::clone(&entry.raw);
        }
        tracing::debug!("cache miss");
        let raw = build().await;
        store_fresh(&mut slot, raw)
    }

    /// Build a fresh raw view under the lock and store it, ignoring the TTL.
    pub(crate) async fn rebuild<F, Fut>(&self, build: F) -> Arc<RawView>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = RawView>,
    {
        let mut slot = self.entries.lock().await;
        let raw = build().await;
        store_fresh(&mut slot, raw)
    }

    /// Apply a marker write to the raw slot under one lock, then return the new
    /// raw view. `Arc::make_mut` mutates the stored view in place when this
    /// task holds the only `Arc`, and clones it on write when a concurrent
    /// reader is still holding a snapshot; either way the reader keeps its
    /// pre-edit data and this task observes the edit. `stored_at` is left
    /// untouched so the TTL still fires on schedule (ADR-0002). A cold or
    /// stale slot is rebuilt from `build`; the fresh build already reflects
    /// the marker on disk and is returned unedited.
    pub(crate) async fn apply_marker_or_build<E, F, Fut>(&self, edit: E, build: F) -> Arc<RawView>
    where
        E: FnOnce(&mut RawView),
        F: FnOnce() -> Fut,
        Fut: Future<Output = RawView>,
    {
        let mut slot = self.entries.lock().await;
        if let Some(entry) = slot.as_mut()
            && is_fresh(entry, self.ttl)
        {
            let raw = Arc::make_mut(&mut entry.raw);
            edit(raw);
            return Arc::clone(&entry.raw);
        }
        let raw = build().await;
        store_fresh(&mut slot, raw)
    }

    /// Rescan one root and replace its section in the raw slot, under one lock,
    /// leaving `stored_at` untouched so the TTL still fires on schedule. Used by
    /// undo: a marker delete can re-flag a subtree, and the in-place edit has
    /// already mutated the cache, so the section is rebuilt by a fresh per-root
    /// scan. A cold or stale slot is rebuilt from `build_full` rather than
    /// spliced into stale neighbors.
    pub(crate) async fn rebuild_root<RS, RsFut, F, Fut>(
        &self,
        root: usize,
        rebuild_section: RS,
        build_full: F,
    ) -> Arc<RawView>
    where
        RS: FnOnce() -> RsFut,
        RsFut: Future<Output = RawRootSection>,
        F: FnOnce() -> Fut,
        Fut: Future<Output = RawView>,
    {
        let mut slot = self.entries.lock().await;
        // is_some_and drops the borrow before the await; an if-let binding would hold it across rebuild_section().await.
        if slot.as_ref().is_some_and(|entry| is_fresh(entry, self.ttl)) {
            let section = rebuild_section().await;
            let entry = slot.as_mut().expect("checked Some above");
            let raw = Arc::make_mut(&mut entry.raw);
            if root < raw.len() {
                raw[root] = section;
            }
            return Arc::clone(&entry.raw);
        }
        let raw = build_full().await;
        store_fresh(&mut slot, raw)
    }

    /// Return a cloned `Arc<RawView>` to the stored raw view, if any.
    ///
    /// Service-layer tests use this to assert that a warm read did not
    /// reseat the cache slot (`Arc::ptr_eq` against a prior snapshot) or
    /// that the slot is populated at all (`.is_some()`). Gated behind
    /// `#[cfg(test)]` so the field stays private at release.
    #[cfg(test)]
    pub(crate) async fn peek_stored_arc(&self) -> Option<Arc<RawView>> {
        let slot = self.entries.lock().await;
        slot.as_ref().map(|entry| Arc::clone(&entry.raw))
    }
}

/// Whether a stored entry is still within the staleness window. `ttl == None`
/// disables the cache (every read rescans), so any stored entry is treated as
/// stale.
fn is_fresh(entry: &CacheEntry, ttl: Option<Duration>) -> bool {
    ttl.is_some_and(|ttl| entry.stored_at.elapsed() < ttl)
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
        let dir_index = Arc::new(StdMutex::new(DirIndex::new()));
        let autosync = crate::autosync::Autosync::new(config.autosync_interval_seconds);
        AppState {
            config: Arc::new(config),
            settings: Arc::new(settings),
            dir_index,
            cache: Cache {
                entries: Mutex::new(None),
                ttl,
            },
            autosync,
        }
    }

    /// Drop every cached directory entry so the next scan walks every
    /// directory from scratch. Reuses `service::lock_index`'s poisoned-lock
    /// recovery: a stale entry is re-listed on its next mtime check, so
    /// wedging future scans is worse than recovering a poisoned lock (commit
    /// `bdcb027`, per ADR-0020).
    pub fn clear_dir_index(&self) {
        let mut guard = crate::service::lock_index(&self.dir_index);
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cache(ttl: Option<Duration>) -> Cache {
        Cache {
            entries: Mutex::new(None),
            ttl,
        }
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

    fn sample_raw(tag: &str) -> RawView {
        vec![RawRootSection {
            path: tag.to_string(),
            state: RawRootState::Clean,
        }]
    }

    #[tokio::test]
    async fn get_or_build_returns_the_stored_raw_within_ttl_unkeyed() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        let first = cache.get_or_build(|| async { sample_raw("first") }).await;
        let second = cache.get_or_build(|| async { sample_raw("second") }).await;
        assert!(
            Arc::ptr_eq(&first, &second),
            "a fresh cache must not rebuild the raw slot"
        );
        assert_eq!(second[0].path, "first");
    }

    #[tokio::test]
    async fn get_or_build_single_flights_a_cold_slot() {
        // Two readers race a cold slot at once. The lock is held across build,
        // so one builds and the other returns the stored view rather than
        // building a second time. This is the guarantee the startup warm leans
        // on to not double-scan a request that races it (see main.rs).
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cache = test_cache(Some(Duration::from_secs(600)));
        let builds = AtomicUsize::new(0);
        let build = || async {
            builds.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            sample_raw("built")
        };
        let (first, second) = tokio::join!(cache.get_or_build(build), cache.get_or_build(build),);
        assert!(
            Arc::ptr_eq(&first, &second),
            "both readers must see the one stored raw view"
        );
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "a cold slot must build exactly once under contention"
        );
    }

    #[tokio::test]
    async fn rebuild_always_builds_and_stores() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        let first = cache.get_or_build(|| async { sample_raw("first") }).await;
        let second = cache.rebuild(|| async { sample_raw("second") }).await;
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second[0].path, "second");
    }

    #[tokio::test]
    async fn apply_marker_or_build_edits_the_raw_slot_under_one_lock() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        cache.get_or_build(|| async { sample_raw("gaps") }).await;
        let returned = cache
            .apply_marker_or_build(
                |raw| raw[0].path = format!("{}-edited", raw[0].path),
                || async { sample_raw("rebuilt") },
            )
            .await;
        assert_eq!(returned[0].path, "gaps-edited");
    }

    #[tokio::test]
    async fn apply_marker_or_build_builds_a_cold_slot() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        let returned = cache
            .apply_marker_or_build(
                |raw| raw[0].path = format!("{}-edited", raw[0].path),
                || async { sample_raw("built") },
            )
            .await;
        // The cold slot was built fresh; the edit closure did not run because
        // the fresh build already reflects the marker on disk.
        assert_eq!(returned[0].path, "built");
    }

    #[tokio::test]
    async fn rebuild_root_replaces_one_section_in_the_raw_slot() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        let two = || {
            vec![
                RawRootSection {
                    path: "keep".to_string(),
                    state: RawRootState::Clean,
                },
                RawRootSection {
                    path: "old".to_string(),
                    state: RawRootState::Clean,
                },
            ]
        };
        cache.get_or_build(|| async { two() }).await;
        let returned = cache
            .rebuild_root(
                1,
                || async {
                    RawRootSection {
                        path: "new".to_string(),
                        state: RawRootState::Clean,
                    }
                },
                || async { two() },
            )
            .await;
        assert_eq!(returned[0].path, "keep");
        assert_eq!(returned[1].path, "new");
    }

    #[tokio::test]
    async fn fresh_build_stamps_stored_at_and_edit_leaves_it() {
        let cache = test_cache(Some(Duration::from_secs(60)));
        cache.get_or_build(|| async { sample_raw("v1") }).await;
        let stamped = {
            let slot = cache.entries.lock().await;
            slot.as_ref().unwrap().stored_at
        };
        cache
            .apply_marker_or_build(
                |raw| raw[0].path = "edited".to_string(),
                || async { sample_raw("unused") },
            )
            .await;
        let after_edit = {
            let slot = cache.entries.lock().await;
            slot.as_ref().unwrap().stored_at
        };
        assert_eq!(stamped, after_edit, "an edit must not bump stored_at");
    }

    #[tokio::test]
    async fn apply_marker_or_build_rebuilds_when_slot_is_stale() {
        // A marker write that arrives after the TTL elapsed must not edit
        // stale raw data: the slot is rebuilt from disk, and the edit closure
        // is skipped because the fresh build already reflects the marker.
        let cache = test_cache(Some(Duration::from_millis(10)));
        cache.get_or_build(|| async { sample_raw("v1") }).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let returned = cache
            .apply_marker_or_build(
                |raw| raw[0].path = format!("{}-edited", raw[0].path),
                || async { sample_raw("rebuilt") },
            )
            .await;
        assert_eq!(
            returned[0].path, "rebuilt",
            "a stale slot must be rebuilt, not edited"
        );
        let after = {
            let slot = cache.entries.lock().await;
            slot.as_ref().unwrap().stored_at.elapsed()
        };
        assert!(
            after < Duration::from_millis(10),
            "stored_at must be refreshed by the rebuild"
        );
    }

    #[tokio::test]
    async fn rebuild_root_rebuilds_full_view_when_slot_is_stale() {
        // The undo path against a stale slot rebuilds the whole view rather
        // than splicing one section into stale neighbors.
        let cache = test_cache(Some(Duration::from_millis(10)));
        let two = || {
            vec![
                RawRootSection {
                    path: "keep".to_string(),
                    state: RawRootState::Clean,
                },
                RawRootSection {
                    path: "old".to_string(),
                    state: RawRootState::Clean,
                },
            ]
        };
        cache.get_or_build(|| async { two() }).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let fresh = || {
            vec![
                RawRootSection {
                    path: "fresh-keep".to_string(),
                    state: RawRootState::Clean,
                },
                RawRootSection {
                    path: "fresh-new".to_string(),
                    state: RawRootState::Clean,
                },
            ]
        };
        let returned = cache
            .rebuild_root(
                1,
                || async {
                    RawRootSection {
                        path: "splice".to_string(),
                        state: RawRootState::Clean,
                    }
                },
                || async { fresh() },
            )
            .await;
        assert_eq!(
            returned[0].path, "fresh-keep",
            "a stale slot must be rebuilt via build_full, not spliced"
        );
        assert_eq!(returned[1].path, "fresh-new");
        let after = {
            let slot = cache.entries.lock().await;
            slot.as_ref().unwrap().stored_at.elapsed()
        };
        assert!(
            after < Duration::from_millis(10),
            "stored_at must be refreshed by the rebuild"
        );
    }
}
