//! Application state: the immutable `Arc<Config>` and compiled `Arc<ScanSettings>`,
//! plus a TTL-memoized scan cache behind one mutex. The cache stores the raw
//! per-root walk output and the response renders per `ViewMode` on each read
//! (see ADR-0022); a marker write updates the stored raw view in place rather
//! than rewalking (see docs/adr/0002-marker-writes-edit-cache-in-place.md).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::Config;
use crate::marker::Marker;
use crate::scanner::{self, DirIndex, RootScan, ScanSettings};
use crate::service::DomainError;

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
    #[allow(dead_code)] // Cache is deleted in a later commit; this method is no longer wired.
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
    #[allow(dead_code)] // Cache is deleted in a later commit.
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
    #[allow(dead_code)] // Cache is deleted in a later commit.
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

/// The result of a successful `RawViewStore::write_mark`. `created` is false
/// for a re-mark of an already-marked folder; callers use it to suppress the
/// undo toast for no-op marks.
#[derive(Debug)]
pub struct Applied {
    /// The refreshed raw view after the in-place edit (or rebuild on a cold slot).
    pub raw: Arc<RawView>,
    /// True when this call made the file; false for a re-mark of a marked folder.
    pub created: bool,
}

impl RawViewStore {
    /// Write a marker into a folder and update the cached raw view in place,
    /// without a rescan (ADR-0002). The guard and write run on a blocking task;
    /// the cache lock is held only for the in-memory mutation.
    pub async fn write_mark(
        &self,
        root: usize,
        rel: &str,
        marker: Marker,
    ) -> Result<Applied, DomainError> {
        let root_path = self
            .config
            .library_roots
            .get(root)
            .ok_or(DomainError::RootIndex)?
            .clone();
        let rel_owned = rel.to_string();
        let (created, canonical_root) =
            tokio::task::spawn_blocking(move || write_marker(&root_path, &rel_owned, marker))
                .await
                .map_err(|_| {
                    DomainError::WriteFailed(std::io::Error::other("marker write task failed"))
                })??;

        // A self-write may not bump the folder mtime, so force a re-list on rescan.
        self.invalidate_index(&canonical_root, rel);

        let rel_for_edit = rel.to_string();
        let raw = {
            let mut slot = self.entries.lock().await;
            if let Some(entry) = slot.as_mut()
                && is_fresh(entry, self.ttl)
            {
                let raw = Arc::make_mut(&mut entry.raw);
                apply_mark_raw(raw, root, &rel_for_edit, marker);
                Arc::clone(&entry.raw)
            } else {
                let raw = self.build_view().await;
                store_fresh(&mut slot, raw)
            }
        };
        Ok(Applied { raw, created })
    }

    /// Drop the index entry for `rel` under `canonical_root` so the next walk
    /// re-lists it. Used after this process writes or deletes a marker, so the
    /// change is picked up even if the directory mtime resolution would have
    /// hidden the same-tick write. A no-op when the path was never indexed.
    ///
    /// `canonical_root` must already be canonicalized by the caller.
    /// `write_marker` and `delete_marker` do that on `spawn_blocking` and hand
    /// back the result, so the sync `canonicalize` syscall stays off the async
    /// runtime thread.
    fn invalidate_index(&self, canonical_root: &Path, rel: &str) {
        let target = if rel == "." {
            canonical_root.to_path_buf()
        } else {
            canonical_root.join(rel)
        };
        crate::service::lock_index(&self.dir_index).invalidate(&target);
    }

    /// Delete a marker file and refresh the cached view by rescanning the one
    /// affected root (ADR-0002). The guard and delete run on a blocking task;
    /// the cache lock is held only for the per-root rebuild.
    pub async fn remove_mark(
        &self,
        root: usize,
        rel: &str,
        marker: Marker,
    ) -> Result<Arc<RawView>, DomainError> {
        let root_path = self
            .config
            .library_roots
            .get(root)
            .ok_or(DomainError::RootIndex)?
            .clone();
        let rel_owned = rel.to_string();
        let delete_path = root_path.clone();
        let canonical_root =
            tokio::task::spawn_blocking(move || delete_marker(&delete_path, &rel_owned, marker))
                .await
                .map_err(|_| {
                    DomainError::WriteFailed(std::io::Error::other("marker delete task failed"))
                })??;

        // A self-delete may not bump the folder mtime, so force a re-list.
        self.invalidate_index(&canonical_root, rel);

        let raw = {
            let mut slot = self.entries.lock().await;
            if slot.as_ref().is_some_and(|entry| is_fresh(entry, self.ttl)) {
                let section = crate::service::build_section(
                    root_path.clone(),
                    Arc::clone(&self.settings),
                    Arc::clone(&self.dir_index),
                )
                .await;
                let entry = slot.as_mut().expect("checked Some above");
                let raw = Arc::make_mut(&mut entry.raw);
                if root < raw.len() {
                    raw[root] = section;
                }
                Arc::clone(&entry.raw)
            } else {
                let raw = self.build_view().await;
                store_fresh(&mut slot, raw)
            }
        };
        Ok(raw)
    }
}

/// Guard the target and create the marker file, on a blocking task. The root
/// base comes from config, so only `rel` is request-controlled. It is
/// re-validated by canonicalizing the join and confirming it stays inside the
/// root. The open is create-only: `Ok(true)` when this call made the file,
/// `Ok(false)` when it was already there, which keeps a re-mark a no-op and
/// lets undo delete only files it created.
fn write_marker(root: &Path, rel: &str, marker: Marker) -> Result<(bool, PathBuf), DomainError> {
    let started = Instant::now();
    let canonical_root = std::fs::canonicalize(root).map_err(|_| DomainError::TargetMissing)?;
    let target = if rel == "." {
        canonical_root.clone()
    } else {
        canonical_root.join(rel)
    };
    let canonical_target =
        std::fs::canonicalize(&target).map_err(|_| DomainError::TargetMissing)?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(DomainError::OutsideRoots);
    }
    if !canonical_target.is_dir() {
        return Err(DomainError::NotADirectory);
    }
    let created = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(canonical_target.join(marker.filename()))
    {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(DomainError::WriteFailed(e)),
    };
    tracing::debug!(
        rel,
        marker = marker.filename(),
        created,
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "wrote marker"
    );
    Ok((created, canonical_root))
}

/// Guard the target and delete the marker file. The guarded mirror of
/// `write_marker`: same canonicalize-and-stay-inside-the-root check. Undo is
/// tolerant: a missing file or a folder that no longer exists is success,
/// since the intended end state (no marker) already holds. Runs on a blocking
/// task.
fn delete_marker(root: &Path, rel: &str, marker: Marker) -> Result<PathBuf, DomainError> {
    let started = Instant::now();
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(root.to_path_buf()),
        Err(_) => return Err(DomainError::TargetMissing),
    };
    let target = if rel == "." {
        canonical_root.clone()
    } else {
        canonical_root.join(rel)
    };
    let canonical_target = match std::fs::canonicalize(&target) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(canonical_root),
        Err(_) => return Err(DomainError::TargetMissing),
    };
    if !canonical_target.starts_with(&canonical_root) {
        return Err(DomainError::OutsideRoots);
    }
    let removed = match std::fs::remove_file(canonical_target.join(marker.filename())) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(DomainError::WriteFailed(e)),
    };
    tracing::debug!(
        rel,
        marker = marker.filename(),
        removed,
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "removed marker"
    );
    Ok(canonical_root)
}

/// Apply a marker write to the raw view. For a non-root mark with path `P`:
/// every entry whose `rel_path` equals `P` or starts with `P` followed by `/`
/// flips `missing_ebook` to false, and the entry whose `rel_path` equals `P`
/// gains the marker filename in `cover_files`. For a root mark (rel == ".",
/// see ADR-0005): every entry under that root flips `missing_ebook` to false,
/// and the empty-rel-path entry gains the marker filename. Component-aware
/// match: `Author` does not cover `Authority/X`.
pub(crate) fn apply_mark_raw(raw: &mut RawView, root: usize, rel: &str, marker: Marker) {
    let Some(section) = raw.get_mut(root) else {
        return;
    };
    let scanner::RootScan::Walked { folders, .. } = section else {
        // A Failed section has no folders to flip; the next rescan will
        // reflect the marker on disk when the slot rebuilds.
        return;
    };
    if rel == "." {
        for folder in folders.iter_mut() {
            folder.missing_ebook = false;
            if folder.rel_path.as_os_str().is_empty() {
                add_marker(&mut folder.cover_files, marker);
            }
        }
        return;
    }
    let marked = PathBuf::from(rel);
    for folder in folders.iter_mut() {
        if folder.rel_path == marked {
            folder.missing_ebook = false;
            add_marker(&mut folder.cover_files, marker);
        } else if folder.rel_path.starts_with(&marked) {
            // PathBuf::starts_with is component-aware, so Author does not match
            // Authority/X; this is the correctness pin against a naive str cmp.
            folder.missing_ebook = false;
        }
    }
}

fn add_marker(cover_files: &mut Vec<String>, marker: Marker) {
    let name = marker.filename().to_string();
    if !cover_files.iter().any(|existing| existing == &name) {
        cover_files.push(name);
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

    #[tokio::test]
    async fn store_write_mark_edits_the_slot_in_place() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let _warm = store.current().await;
        let applied = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(applied.created);
        let stored = store.peek_stored_arc().await.unwrap();
        assert!(
            Arc::ptr_eq(&stored, &applied.raw),
            "slot stores the edited view"
        );
    }

    #[tokio::test]
    async fn store_write_mark_idempotent_create_false_on_second_call() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let first = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(first.created);
        let second = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(!second.created);
    }

    #[tokio::test]
    async fn store_write_mark_bad_root_index_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let err = store.write_mark(9, ".", Marker::NoEbook).await.unwrap_err();
        assert!(matches!(err, DomainError::RootIndex));
    }

    // Direct unit tests for the private `write_marker` helper. They live here
    // (not in service.rs) because the helper moved into this module.

    #[test]
    fn write_marker_creates_each_marker_file() {
        for marker in Marker::ALL {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("Book")).unwrap();
            write_marker(dir.path(), "Book", marker).unwrap();
            assert!(dir.path().join("Book").join(marker.filename()).exists());
        }
    }

    #[test]
    fn write_marker_reports_created_then_not_created() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Book")).unwrap();
        assert!(write_marker(dir.path(), "Book", Marker::NoEbook).unwrap().0);
        assert!(!write_marker(dir.path(), "Book", Marker::NoEbook).unwrap().0);
        assert!(dir.path().join("Book").join(".no_ebook").exists());
    }

    #[test]
    fn write_marker_at_the_root_uses_dot() {
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path(), ".", Marker::NoEbook).unwrap();
        assert!(dir.path().join(".no_ebook").exists());
    }

    #[test]
    fn write_marker_rejects_an_escape() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_marker(dir.path(), "..", Marker::NoEbook).unwrap_err();
        assert!(matches!(err, DomainError::OutsideRoots));
    }

    #[test]
    fn write_marker_missing_target_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_marker(dir.path(), "Nope", Marker::NoEbook).unwrap_err();
        assert!(matches!(err, DomainError::TargetMissing));
    }

    #[test]
    fn write_marker_rejects_a_file_target() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let err = write_marker(dir.path(), "Book/01.mp3", Marker::NoEbook).unwrap_err();
        assert!(matches!(err, DomainError::NotADirectory));
    }

    #[test]
    fn delete_marker_removes_each_marker_file() {
        for marker in Marker::ALL {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("Book")).unwrap();
            let path = dir.path().join("Book").join(marker.filename());
            std::fs::write(&path, b"").unwrap();
            delete_marker(dir.path(), "Book", marker).unwrap();
            assert!(!path.exists());
        }
    }

    #[test]
    fn delete_marker_rejects_an_escape() {
        let dir = tempfile::tempdir().unwrap();
        let err = delete_marker(dir.path(), "..", Marker::NoEbook).unwrap_err();
        assert!(matches!(err, DomainError::OutsideRoots));
    }

    #[test]
    fn delete_marker_is_tolerant_of_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Book")).unwrap();
        delete_marker(dir.path(), "Book", Marker::NoEbook).unwrap();
    }

    #[test]
    fn delete_marker_is_tolerant_of_a_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        delete_marker(dir.path(), "Gone", Marker::NoEbook).unwrap();
    }

    #[tokio::test]
    async fn store_remove_mark_re_flags_the_root() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let _ = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(dir.path().join("Book/.no_ebook").exists());

        let after = store.remove_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(!dir.path().join("Book/.no_ebook").exists());
        let scanner::RootScan::Walked { folders, .. } = &after[0] else {
            panic!("expected Walked");
        };
        let book = folders
            .iter()
            .find(|f| f.rel_path.to_str() == Some("Book"))
            .unwrap();
        assert!(book.missing_ebook, "remove_mark re-flagged the folder");
    }
}
