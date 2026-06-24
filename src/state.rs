//! Application state: the immutable `Arc<Config>` and compiled `Arc<ScanSettings>`,
//! plus a TTL-memoized scan cache behind one mutex. The cache stores the raw
//! per-root walk output and the response renders per `ViewMode` on each read
//! (see ADR-0022); a marker write updates the stored raw view in place rather
//! than rewalking (see docs/adr/0002-marker-writes-edit-cache-in-place.md).

use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::Config;
use crate::scanner::{DirIndex, RootScan, ScanSettings};

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
    // Wired into service/web/autosync in subsequent commits; field exists
    // first so `AppState::new` can construct the store from shared Arcs.
    #[allow(dead_code)]
    pub(crate) store: RawViewStore,
    /// The autosync subscriber registry and loop handle. The loop spawns on the
    /// first SSE subscription with a non-zero `autosync_interval_seconds` and
    /// exits when the last subscriber disconnects (ADR-0023).
    pub(crate) autosync: crate::autosync::Autosync,
}

/// The whole raw view: one `RootScan` per configured library root, in config order.
pub type RawView = Vec<RootScan>;

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
        RsFut: Future<Output = RootScan>,
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

/// Owns the scan substrate, the TTL-bounded cache slot, and the marker file
/// IO. The single place where raw scan output is produced, memoized, and
/// edited. See ADR-0027.
pub struct RawViewStore {
    /// The cache slot. Held briefly for in-place edits; held across the
    /// per-root rescan in `remove_mark` (matching today's `rebuild_root`).
    entries: Mutex<Option<CacheEntry>>,
    /// `None` disables caching: every read rescans.
    ttl: Option<Duration>,
    /// Scan substrate: the compiled settings and the shared mtime index.
    settings: Arc<ScanSettings>,
    dir_index: Arc<StdMutex<DirIndex>>,
    /// Held by the store for `build_view`; the same `Arc<Config>` is also
    /// exposed on `AppState.config` for handlers that read pure config data
    /// (search links, cookie name, library roots). See ADR-0027.
    config: Arc<Config>,
}

impl RawViewStore {
    /// Build a fresh store. `ttl == None` disables caching so every read
    /// rescans; any other value sets the staleness window.
    pub fn new(
        config: Arc<Config>,
        settings: Arc<ScanSettings>,
        dir_index: Arc<StdMutex<DirIndex>>,
        ttl: Option<Duration>,
    ) -> RawViewStore {
        RawViewStore {
            entries: Mutex::new(None),
            ttl,
            settings,
            dir_index,
            config,
        }
    }

    /// Return the cached raw view if still fresh, otherwise build one under the
    /// lock and store it. Single-flight. TTL-respecting. Used by page loads
    /// and the SSE first event.
    pub async fn current(&self) -> Arc<RawView> {
        let mut slot = self.entries.lock().await;
        if let Some(entry) = slot.as_ref()
            && is_fresh(entry, self.ttl)
        {
            tracing::debug!("cache hit");
            return Arc::clone(&entry.raw);
        }
        tracing::debug!("cache miss");
        let raw = self.build_view().await;
        store_fresh(&mut slot, raw)
    }

    /// Rebuild under the lock and store, ignoring the TTL but keeping the dir
    /// index. The autosync loop calls this each tick to pick up filesystem
    /// changes without forcing a cold walk.
    pub async fn refresh(&self) -> Arc<RawView> {
        let mut slot = self.entries.lock().await;
        let raw = self.build_view().await;
        store_fresh(&mut slot, raw)
    }

    /// Force a fresh cold scan: clear the dir index, build under the lock,
    /// store, return. Ignores the TTL. The explicit "fix any drift" path,
    /// used by the /rescan click.
    pub async fn rescan(&self) -> Arc<RawView> {
        crate::service::lock_index(&self.dir_index).clear();
        let mut slot = self.entries.lock().await;
        let raw = self.build_view().await;
        store_fresh(&mut slot, raw)
    }

    /// Build the raw view for every configured root, in config order.
    async fn build_view(&self) -> RawView {
        crate::service::build_view(
            self.config.as_ref(),
            &self.settings,
            Arc::clone(&self.dir_index),
        )
        .await
    }

    /// Test accessor: returns a cloned `Arc<RawView>` of the stored slot, if any.
    /// Used in tests to assert that a warm read did not reseat the slot
    /// (`Arc::ptr_eq` against a prior snapshot).
    #[cfg(test)]
    pub async fn peek_stored_arc(&self) -> Option<Arc<RawView>> {
        let slot = self.entries.lock().await;
        slot.as_ref().map(|entry| Arc::clone(&entry.raw))
    }
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
        let config = Arc::new(config);
        let settings = Arc::new(settings);
        let dir_index = Arc::new(StdMutex::new(DirIndex::new()));
        let autosync = crate::autosync::Autosync::new(config.autosync_interval_seconds);
        let store = RawViewStore::new(
            Arc::clone(&config),
            Arc::clone(&settings),
            Arc::clone(&dir_index),
            ttl,
        );
        AppState {
            config,
            settings,
            dir_index,
            cache: Cache {
                entries: Mutex::new(None),
                ttl,
            },
            store,
            autosync,
        }
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
        vec![walked(tag)]
    }

    fn walked(tag: &str) -> RootScan {
        RootScan::Walked {
            canonical_path: std::path::PathBuf::from(tag),
            folders: Vec::new(),
        }
    }

    fn path_of(scan: &RootScan) -> String {
        scan.display_path().to_string()
    }

    fn rename(scan: &mut RootScan, new: &str) {
        if let RootScan::Walked { canonical_path, .. } = scan {
            *canonical_path = std::path::PathBuf::from(new);
        }
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
        assert_eq!(path_of(&second[0]), "first");
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
        assert_eq!(path_of(&second[0]), "second");
    }

    #[tokio::test]
    async fn apply_marker_or_build_edits_the_raw_slot_under_one_lock() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        cache.get_or_build(|| async { sample_raw("gaps") }).await;
        let returned = cache
            .apply_marker_or_build(
                |raw| {
                    let renamed = format!("{}-edited", path_of(&raw[0]));
                    rename(&mut raw[0], &renamed);
                },
                || async { sample_raw("rebuilt") },
            )
            .await;
        assert_eq!(path_of(&returned[0]), "gaps-edited");
    }

    #[tokio::test]
    async fn apply_marker_or_build_builds_a_cold_slot() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        let returned = cache
            .apply_marker_or_build(
                |raw| {
                    let renamed = format!("{}-edited", path_of(&raw[0]));
                    rename(&mut raw[0], &renamed);
                },
                || async { sample_raw("built") },
            )
            .await;
        // The cold slot was built fresh; the edit closure did not run because
        // the fresh build already reflects the marker on disk.
        assert_eq!(path_of(&returned[0]), "built");
    }

    #[tokio::test]
    async fn rebuild_root_replaces_one_section_in_the_raw_slot() {
        let cache = test_cache(Some(Duration::from_secs(600)));
        let two = || vec![walked("keep"), walked("old")];
        cache.get_or_build(|| async { two() }).await;
        let returned = cache
            .rebuild_root(1, || async { walked("new") }, || async { two() })
            .await;
        assert_eq!(path_of(&returned[0]), "keep");
        assert_eq!(path_of(&returned[1]), "new");
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
                |raw| rename(&mut raw[0], "edited"),
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
                |raw| {
                    let renamed = format!("{}-edited", path_of(&raw[0]));
                    rename(&mut raw[0], &renamed);
                },
                || async { sample_raw("rebuilt") },
            )
            .await;
        assert_eq!(
            path_of(&returned[0]),
            "rebuilt",
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
        let two = || vec![walked("keep"), walked("old")];
        cache.get_or_build(|| async { two() }).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let fresh = || vec![walked("fresh-keep"), walked("fresh-new")];
        let returned = cache
            .rebuild_root(1, || async { walked("splice") }, || async { fresh() })
            .await;
        assert_eq!(
            path_of(&returned[0]),
            "fresh-keep",
            "a stale slot must be rebuilt via build_full, not spliced"
        );
        assert_eq!(path_of(&returned[1]), "fresh-new");
        let after = {
            let slot = cache.entries.lock().await;
            slot.as_ref().unwrap().stored_at.elapsed()
        };
        assert!(
            after < Duration::from_millis(10),
            "stored_at must be refreshed by the rebuild"
        );
    }

    fn test_store(ttl: Option<Duration>, root: std::path::PathBuf) -> RawViewStore {
        let cfg = Config {
            library_roots: vec![root],
            ttl_seconds: ttl.map(|t| t.as_secs()).unwrap_or(0),
            ..Default::default()
        };
        let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
        let dir_index = Arc::new(StdMutex::new(DirIndex::new()));
        RawViewStore::new(Arc::new(cfg), Arc::new(settings), dir_index, ttl)
    }

    #[tokio::test]
    async fn store_current_serves_stored_raw_within_ttl() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let first = store.current().await;
        crate::scenarios::touch(&dir.path().join("Book/Book.epub"));
        let second = store.current().await;

        assert!(Arc::ptr_eq(&first, &second), "warm store must not rebuild");
    }

    #[tokio::test]
    async fn store_current_single_flights_a_cold_slot() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = Arc::new(test_store(
            Some(Duration::from_secs(600)),
            dir.path().to_path_buf(),
        ));
        let s1 = Arc::clone(&store);
        let s2 = Arc::clone(&store);
        let (a, b) = tokio::join!(s1.current(), s2.current());
        assert!(Arc::ptr_eq(&a, &b), "single-flight: one Arc shared");
    }

    #[tokio::test]
    async fn store_rescan_clears_the_dir_index() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let _ = store.current().await;
        let before = store.dir_index.lock().unwrap().len();
        assert!(before > 0);
        let _ = store.rescan().await;
        let after = store.dir_index.lock().unwrap().len();
        assert!(after > 0, "rescan repopulates the index after clearing it");
    }
}
