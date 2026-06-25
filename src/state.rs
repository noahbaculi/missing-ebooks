//! Application state: the immutable `Arc<Config>` and compiled `Arc<ScanSettings>`,
//! plus a TTL-memoized scan cache behind one mutex. The cache stores the raw
//! per-root walk output and the response renders per `ViewMode` on each read
//! (see ADR-0022); a marker write updates the stored raw view in place rather
//! than rewalking (see docs/adr/0002-marker-writes-edit-cache-in-place.md).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::Config;
use crate::marker::Marker;
use crate::raw_view::{RawView, apply_mark_raw, build_section, build_view, lock_index};
use crate::scanner::{DirIndex, ScanSettings};

/// Everything a request handler needs: the immutable config and settings, the
/// scan cache, and the autosync registry. Shared as `Arc<AppState>`.
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) store: RawViewStore,
    /// The autosync subscriber registry and loop handle. The loop spawns on the
    /// first SSE subscription with a non-zero `autosync_interval_seconds` and
    /// exits when the last subscriber disconnects (ADR-0023).
    pub(crate) autosync: crate::autosync::Autosync,
}

/// A stored raw view and the instant it was scanned. Owned by `RawViewStore`'s
/// cache slot.
struct CacheEntry {
    stored_at: Instant,
    raw: Arc<RawView>,
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
    /// Monotonic count of fresh builds stored into the slot. Bumped inside
    /// `store_fresh`. Test-only observation; tests diff before vs. after to
    /// assert that a warm operation did not rebuild. See ADR-0022.
    rebuild_count: AtomicU64,
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
            rebuild_count: AtomicU64::new(0),
            ttl,
            settings,
            dir_index,
            config,
        }
    }

    /// Stamp and store a freshly built raw view, bumping `rebuild_count`.
    /// The single place that sets `stored_at = now`: a fresh build refreshes
    /// the freshness clock (ADR-0002). `write_mark` uses `Arc::make_mut` for
    /// in-place edits and intentionally bypasses this method, leaving both
    /// `stored_at` and `rebuild_count` unchanged.
    fn store_fresh(&self, slot: &mut Option<CacheEntry>, raw: RawView) -> Arc<RawView> {
        let raw = Arc::new(raw);
        *slot = Some(CacheEntry {
            stored_at: Instant::now(),
            raw: Arc::clone(&raw),
        });
        self.rebuild_count.fetch_add(1, Ordering::Relaxed);
        raw
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
        self.store_fresh(&mut slot, raw)
    }

    /// Rebuild under the lock and store, ignoring the TTL but keeping the dir
    /// index. The autosync loop calls this each tick to pick up filesystem
    /// changes without forcing a cold walk.
    pub async fn refresh(&self) -> Arc<RawView> {
        let mut slot = self.entries.lock().await;
        let raw = self.build_view().await;
        self.store_fresh(&mut slot, raw)
    }

    /// Force a fresh cold scan: clear the dir index, build under the lock,
    /// store, return. Ignores the TTL. The explicit "fix any drift" path,
    /// used by the /rescan click.
    pub async fn rescan(&self) -> Arc<RawView> {
        lock_index(&self.dir_index).clear();
        let mut slot = self.entries.lock().await;
        let raw = self.build_view().await;
        self.store_fresh(&mut slot, raw)
    }

    /// Build the raw view for every configured root, in config order.
    async fn build_view(&self) -> RawView {
        build_view(
            self.config.as_ref(),
            &self.settings,
            Arc::clone(&self.dir_index),
        )
        .await
    }

    /// Returns the count of fresh builds stored into the slot since this
    /// store was created.
    #[cfg(test)]
    pub fn rebuild_count(&self) -> u64 {
        self.rebuild_count.load(Ordering::Relaxed)
    }

    /// Test accessor: returns the shared dir index. Used in tests that need
    /// to insert synthetic entries or assert on the index's content.
    #[cfg(test)]
    pub fn dir_index(&self) -> &Arc<StdMutex<DirIndex>> {
        &self.dir_index
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
        let autosync = crate::autosync::Autosync::new(config.autosync_interval_seconds);
        let store = RawViewStore::new(
            Arc::clone(&config),
            Arc::new(settings),
            Arc::new(StdMutex::new(DirIndex::new())),
            ttl,
        );
        AppState {
            config,
            store,
            autosync,
        }
    }

    /// Warm the cache slot by reading the current raw view. Used by the binary
    /// crate's startup spawn so the first viewer after a restart does not pay
    /// the cold scan. The returned `Arc` is discarded by the caller; the
    /// side effect on the cache slot is what we want.
    pub async fn warm(&self) {
        let _ = self.store.current().await;
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

/// A failure performing a write action. The HTML surface renders it inline. A
/// future JSON API would render it as an error body.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    /// The submitted root index does not name a configured root.
    #[error("no such library root")]
    RootIndex,
    /// The resolved target sits outside every configured root.
    #[error("target is outside the configured library roots")]
    OutsideRoots,
    /// The target folder does not exist, or could not be canonicalized.
    #[error("target folder does not exist")]
    TargetMissing,
    /// The target resolved to a file rather than a directory.
    #[error("target is not a directory")]
    NotADirectory,
    /// The marker file could not be written.
    #[error("could not write the marker file: {0}")]
    WriteFailed(std::io::Error),
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
                self.store_fresh(&mut slot, raw)
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
        lock_index(&self.dir_index).invalidate(&target);
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
                let section = build_section(
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
                self.store_fresh(&mut slot, raw)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{self, RootScan};

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
        assert_eq!(state.store.ttl, None);
    }

    #[test]
    fn nonzero_ttl_sets_the_window() {
        let cfg = Config {
            ttl_seconds: 90,
            ..Default::default()
        };
        let state = AppState::new(cfg, settings());
        assert_eq!(state.store.ttl, Some(Duration::from_secs(90)));
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

        let _warm = store.current().await;
        let rebuilds_before = store.rebuild_count();
        crate::scenarios::touch(&dir.path().join("Book/Book.epub"));
        let _again = store.current().await;

        assert_eq!(
            store.rebuild_count(),
            rebuilds_before,
            "warm read must not rebuild the slot",
        );
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
        let (_a, _b) = tokio::join!(s1.current(), s2.current());
        assert_eq!(
            store.rebuild_count(),
            1,
            "single-flight: one rebuild for two concurrent cold reads",
        );
    }

    #[tokio::test]
    async fn store_rescan_clears_the_dir_index_then_repopulates_it() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        // Warm the index by reading once.
        let _ = store.current().await;
        let after_warm = store.dir_index.lock().unwrap().len();
        assert!(after_warm > 0, "the warm read populated the dir index");

        // Drop a synthetic entry into the index that no real walk could reach.
        // A cold rescan must drop it; a warm rescan would preserve it.
        let synthetic_path = std::path::PathBuf::from("/nonexistent/synthetic/marker/path");
        store.dir_index.lock().unwrap().insert(
            synthetic_path.clone(),
            scanner::CachedDir {
                mtime: std::time::UNIX_EPOCH,
                subdirs: Vec::new(),
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            },
        );
        assert!(
            store
                .dir_index
                .lock()
                .unwrap()
                .get(&synthetic_path)
                .is_some()
        );

        // Rescan must drop every entry, then the rebuild repopulates it.
        let _ = store.rescan().await;
        assert!(
            store
                .dir_index
                .lock()
                .unwrap()
                .get(&synthetic_path)
                .is_none(),
            "rescan must clear the dir index, dropping the synthetic entry"
        );
        let after_rescan = store.dir_index.lock().unwrap().len();
        assert_eq!(
            after_rescan, after_warm,
            "rescan must clear and repopulate to the same count on an unchanged tree"
        );
    }

    #[tokio::test]
    async fn store_write_mark_edits_the_slot_in_place() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let _warm = store.current().await;
        let rebuilds_before = store.rebuild_count();
        let applied = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(applied.created);
        assert_eq!(
            store.rebuild_count(),
            rebuilds_before,
            "warm write_mark must not rebuild",
        );
        assert!(!book_missing(&applied.raw), "the edit is reflected");

        let _next = store.current().await;
        assert_eq!(
            store.rebuild_count(),
            rebuilds_before,
            "follow-up read must not rebuild",
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

    /// Helper: assert that `Book` under root 0 of `raw` has the expected
    /// `missing_ebook` value. Used by ported tests that previously asserted on
    /// the packaged `RootState`; the equivalent at the raw layer reads off the
    /// matching `ScannedFolder` so the assertion stays at the store's layer.
    fn book_missing(raw: &RawView) -> bool {
        let RootScan::Walked { folders, .. } = &raw[0] else {
            panic!("expected Walked root");
        };
        folders
            .iter()
            .find(|f| f.rel_path.as_os_str() == "Book")
            .expect("Book folder in raw view")
            .missing_ebook
    }

    #[tokio::test]
    async fn store_warm_concurrent_reads_share_one_raw_slot() {
        // Warm reads against a single slot must not rebuild, matching the
        // property ADR-0022 documents for warm reads. The cold single-flight
        // case is `store_current_single_flights_a_cold_slot`.
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = Arc::new(test_store(
            Some(Duration::from_secs(600)),
            dir.path().to_path_buf(),
        ));

        let _warm = store.current().await;
        let rebuilds_before = store.rebuild_count();

        let s1 = Arc::clone(&store);
        let s2 = Arc::clone(&store);
        let (_a, _b) = tokio::join!(s1.current(), s2.current());

        assert_eq!(
            store.rebuild_count(),
            rebuilds_before,
            "warm concurrent reads must not rebuild",
        );
    }

    #[tokio::test]
    async fn store_ttl_zero_rescans_every_call() {
        use filetime::{FileTime, set_file_mtime};
        let dir = tempfile::tempdir().unwrap();
        let book = dir.path().join("Book");
        crate::scenarios::touch(&book.join("01.mp3"));
        let store = test_store(None, dir.path().to_path_buf());

        let first = store.current().await;
        assert!(book_missing(&first), "first read sees the gap");

        // Cover the gap, then push the folder mtime forward so the rescan sees
        // the change regardless of the filesystem's mtime resolution. The dir
        // index keys off mtime equality; back-to-back touches inside one tick
        // would otherwise reuse the pre-cover listing and hide the new ebook.
        crate::scenarios::touch(&book.join("Book.epub"));
        set_file_mtime(&book, FileTime::from_unix_time(4_000_000_000, 0)).unwrap();
        let second = store.current().await;
        assert!(!book_missing(&second), "ttl 0 rescanned and saw the cover");
    }

    #[tokio::test]
    async fn store_rescan_refreshes_even_within_a_live_ttl() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let first = store.current().await;
        assert!(book_missing(&first));

        crate::scenarios::touch(&dir.path().join("Book/Book.epub"));
        let refreshed = store.rescan().await;
        assert!(
            !book_missing(&refreshed),
            "rescan bypasses the live TTL and sees the cover"
        );
    }

    #[tokio::test]
    async fn store_ttl_zero_keeps_the_dir_index_warm() {
        // ttl 0 rescans on every read, but the dir index survives across
        // reads (ADR-0023). Two reads in a row should both see the gap and
        // leave the index populated.
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Author/Book/01.mp3"));
        let store = test_store(None, dir.path().to_path_buf());

        let first = store.current().await;
        let second = store.current().await;
        let RootScan::Walked { folders: f1, .. } = &first[0] else {
            panic!("expected Walked");
        };
        let RootScan::Walked { folders: f2, .. } = &second[0] else {
            panic!("expected Walked");
        };
        assert!(f1.iter().any(|f| f.missing_ebook));
        assert!(f2.iter().any(|f| f.missing_ebook));

        let reused = store.dir_index.lock().unwrap().len();
        assert!(reused > 0, "the index retained the walked directories");
    }

    #[tokio::test]
    async fn store_write_mark_invalidates_the_marked_dir_in_the_index() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        // Warm the index by scanning once.
        store.current().await;
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let book = canonical.join("Book");
        assert!(
            store.dir_index.lock().unwrap().get(&book).is_some(),
            "Book is indexed after the scan"
        );

        // Marking Book writes .no_ebook into it, so its index entry must be dropped
        // and the next walk re-lists it rather than trusting a pre-write mtime.
        store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(
            store.dir_index.lock().unwrap().get(&book).is_none(),
            "Book's index entry is invalidated by the marker write"
        );
    }

    #[tokio::test]
    async fn store_write_mark_warm_slot_survives_follow_up_read() {
        // The in-place edit must persist across a follow-up read inside the
        // live TTL: the slot is not rebuilt and the second read reflects the
        // mark.
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let _first = store.current().await;
        let rebuilds_before = store.rebuild_count();
        let applied = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(!book_missing(&applied.raw));
        assert_eq!(
            store.rebuild_count(),
            rebuilds_before,
            "warm write_mark must not rebuild",
        );

        // A new gap appears on disk, but the warm TTL means the next read
        // serves from the cached raw slot rather than rescanning.
        crate::scenarios::touch(&dir.path().join("Other/01.mp3"));
        let again = store.current().await;
        assert_eq!(
            store.rebuild_count(),
            rebuilds_before,
            "follow-up read must not rebuild the slot",
        );
        assert!(!book_missing(&again), "follow-up read reflects the mark");
    }

    #[tokio::test]
    async fn store_write_mark_on_a_cold_cache_scans_fresh() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let applied = store
            .write_mark(0, "Book", Marker::EbookElsewhere)
            .await
            .unwrap();
        assert!(applied.created);
        assert!(!book_missing(&applied.raw));
        assert!(dir.path().join("Book/.ebook_elsewhere").exists());
    }

    #[tokio::test]
    async fn store_write_mark_outside_a_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let err = store
            .write_mark(0, "..", Marker::NoEbook)
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::OutsideRoots));
    }

    #[tokio::test]
    async fn store_remove_mark_bad_root_index_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let err = store
            .remove_mark(9, ".", Marker::NoEbook)
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::RootIndex));
    }
}
