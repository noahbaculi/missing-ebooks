//! Application state: the immutable `Arc<Config>` and compiled `Arc<ScanSettings>`,
//! plus a TTL-memoized scan cache behind one mutex. The cache stores the raw
//! per-root walk output and the response renders per `ViewMode` on each read
//! (see ADR-0022). A marker write updates the stored raw view in place rather
//! than rewalking (see docs/adr/0002-marker-writes-edit-cache-in-place.md).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use tokio::sync::Mutex;

use crate::config::Config;
use crate::marker::Marker;
use crate::raw_view::{RawView, apply_mark_raw, build_section, build_view};
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

/// A coalesced cold build, shared by every caller that arrives while it runs.
type SharedBuild = Shared<BoxFuture<'static, Arc<RawView>>>;

/// Owns the scan substrate, the TTL-bounded cache slot, and the marker file
/// IO. The single place where raw scan output is produced, memoized, and
/// edited. See ADR-0027.
pub struct RawViewStore {
    /// The cache slot, swapped atomically. Reads are lock-free: `current`
    /// loads the `Arc<CacheEntry>` and clones the inner `Arc<RawView>` out.
    cache: ArcSwapOption<CacheEntry>,
    /// Holds only the in-flight build handle (as a `Weak`, so it expires when
    /// the last awaiter drops it). Held for microseconds to register or join
    /// a build, never across the walk. Also serializes marker edits so a
    /// concurrent `write_mark` / `remove_mark` cannot interleave its
    /// load-edit-store with a cold build's store.
    inflight: Mutex<Option<Weak<SharedBuild>>>,
    /// Monotonic count of fresh builds stored into the slot. Bumped inside
    /// `store_fresh` as a test-only observation. Tests diff before vs. after
    /// to assert that a warm operation did not rebuild. See ADR-0022.
    rebuild_count: AtomicU64,
    /// `None` disables caching: every read rescans.
    ttl: Option<Duration>,
    /// Scan substrate: the compiled settings and the shared mtime index.
    settings: Arc<ScanSettings>,
    /// Per-root mtime indices. One persistent `DirIndex` per configured
    /// library root, so the per-root walks in `build_view` hold disjoint
    /// locks and run truly in parallel. The lock now lives inside
    /// `DirIndex`. The store holds a shared reference per root.
    dir_indices: Vec<Arc<DirIndex>>,
    /// Held by the store for `build_view`. The same `Arc<Config>` is also
    /// exposed on `AppState.config` for handlers that read pure config data
    /// (search links, cookie name, library roots). See ADR-0027.
    config: Arc<Config>,
}

impl RawViewStore {
    /// Build a fresh store. `ttl == None` disables caching so every read
    /// rescans. Any other value sets the staleness window.
    pub fn new(
        config: Arc<Config>,
        settings: Arc<ScanSettings>,
        dir_indices: Vec<Arc<DirIndex>>,
        ttl: Option<Duration>,
    ) -> RawViewStore {
        RawViewStore {
            cache: ArcSwapOption::empty(),
            inflight: Mutex::new(None),
            rebuild_count: AtomicU64::new(0),
            ttl,
            settings,
            dir_indices,
            config,
        }
    }

    /// Stamp and store a freshly built raw view, bumping `rebuild_count`. The
    /// single place that sets `stored_at = now`. The in-place marker edit
    /// stores directly (preserving `stored_at`) and never calls this, so a
    /// warm write leaves both the freshness clock and the rebuild count
    /// unchanged.
    fn store_fresh(&self, raw: Arc<RawView>) -> Arc<RawView> {
        self.cache.store(Some(Arc::new(CacheEntry {
            stored_at: Instant::now(),
            raw: Arc::clone(&raw),
        })));
        self.rebuild_count.fetch_add(1, Ordering::Relaxed);
        raw
    }

    /// Return the cached raw view if still fresh, otherwise build one. Reads
    /// are lock-free. Concurrent cold reads coalesce onto one build.
    /// TTL-respecting. Used by page loads and the SSE first event.
    pub async fn current(&self) -> Arc<RawView> {
        if let Some(entry) = self.cache.load_full()
            && is_fresh(&entry, self.ttl)
        {
            tracing::debug!("cache hit");
            return Arc::clone(&entry.raw);
        }
        tracing::debug!("cache miss");
        self.build_coalesced(true).await
    }

    /// Start a build, or join the one already running. The first caller owns
    /// the build and stores the result (bumping `rebuild_count` once).
    /// Joiners share the same future and return its output without a second
    /// walk or a second count bump.
    ///
    /// `recheck_fresh`: when true (the `current` read path), re-check the cache
    /// under the lock and return a fresh entry instead of starting a new walk.
    /// This closes the cold-burst race where the owning build stored a fresh
    /// entry and dropped its in-flight handle while this caller waited for the
    /// lock, which would otherwise trigger a redundant second full walk. The
    /// `refresh`/`rescan`/edit-fallback callers pass false: they must rebuild
    /// regardless of a live TTL.
    async fn build_coalesced(&self, recheck_fresh: bool) -> Arc<RawView> {
        let (handle, owns) = {
            let mut slot = self.inflight.lock().await;
            if recheck_fresh
                && let Some(entry) = self.cache.load_full()
                && is_fresh(&entry, self.ttl)
            {
                return Arc::clone(&entry.raw);
            }
            if let Some(existing) = slot.as_ref().and_then(Weak::upgrade) {
                (existing, false)
            } else {
                let config = Arc::clone(&self.config);
                let settings = Arc::clone(&self.settings);
                let indices: Vec<_> = self.dir_indices.iter().map(Arc::clone).collect();
                let build: SharedBuild =
                    async move { Arc::new(build_view(&config, &settings, &indices).await) }
                        .boxed()
                        .shared();
                let handle = Arc::new(build);
                *slot = Some(Arc::downgrade(&handle));
                (handle, true)
            }
        };
        // Keep the Arc alive across the await so a concurrent caller can
        // upgrade the Weak and join this build. Cloning the inner Shared
        // future is what actually yields the awaitable.
        let raw = (*handle).clone().await;
        if owns {
            self.store_fresh(Arc::clone(&raw));
        }
        raw
    }

    /// Rebuild ignoring the TTL, keeping the dir index. The autosync tick
    /// path: the loop calls this each tick to pick up filesystem changes
    /// without forcing a cold walk.
    pub(crate) async fn refresh(&self) -> Arc<RawView> {
        self.build_coalesced(false).await
    }

    /// Force a cold scan: clear every per-root index, then rebuild. Ignores
    /// the TTL. The explicit "fix any drift" path, used by the /rescan click.
    pub(crate) async fn rescan(&self) -> Arc<RawView> {
        for index in &self.dir_indices {
            index.clear();
        }
        self.build_coalesced(false).await
    }

    /// Build the raw view for every configured root, in config order.
    async fn build_view(&self) -> RawView {
        build_view(self.config.as_ref(), &self.settings, &self.dir_indices).await
    }

    /// Returns the count of fresh builds stored into the slot since this
    /// store was created.
    #[cfg(test)]
    pub fn rebuild_count(&self) -> u64 {
        self.rebuild_count.load(Ordering::Relaxed)
    }

    /// Test-only seam onto one root's persistent `DirIndex` entry count, so
    /// tests can assert that a build path did not clear the per-root mtime
    /// cache.
    #[cfg(test)]
    pub(crate) fn dir_index_len_for_test(&self, root: usize) -> usize {
        self.dir_indices[root].len()
    }
}

impl AppState {
    /// Build the shared state. `ttl_seconds == 0` disables the cache. Any other
    /// value sets the staleness window.
    pub fn new(config: Config, settings: ScanSettings) -> AppState {
        let ttl = if config.ttl_seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(config.ttl_seconds))
        };
        let config = Arc::new(config);
        let autosync = crate::autosync::Autosync::new(config.autosync_interval_seconds);
        let dir_indices = (0..config.library_roots.len())
            .map(|_| Arc::new(DirIndex::new()))
            .collect();
        let store = RawViewStore::new(Arc::clone(&config), Arc::new(settings), dir_indices, ttl);
        AppState {
            config,
            store,
            autosync,
        }
    }

    /// Warm the cache slot by reading the current raw view. Used by the binary
    /// crate's startup spawn so the first viewer after a restart does not pay
    /// the cold scan. The returned `Arc` is discarded by the caller. The
    /// side effect on the cache slot is the point.
    pub async fn warm(&self) {
        let _ = self.store.current().await;
    }
}

/// The result of a successful `RawViewStore::write_mark`. `created` is false
/// for a re-mark of an already-marked folder. Callers use it to suppress the
/// undo toast for no-op marks.
#[derive(Debug)]
pub(crate) struct Applied {
    /// The refreshed raw view after the in-place edit (or rebuild on a cold slot).
    pub raw: Arc<RawView>,
    /// True when this call made the file, false for a re-mark of a marked folder.
    pub created: bool,
}

/// A failure inside the write attempt: the row was looked up but the marker
/// file could not be written or deleted. `RawViewStore` surfaces these via
/// `WriteFailure::Failed` alongside the still-valid raw view.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WriteError {
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

/// The shape returned by `RawViewStore::write_mark` and `remove_mark`.
/// `BadRoot` is the pre-write bail when the submitted root index is out
/// of range. `Failed` carries the current raw view so the caller can
/// render an inline alert by the affected row without a second store hop.
#[derive(Debug)]
pub(crate) enum WriteFailure {
    /// Pre-write bail: the submitted root index does not name a configured
    /// root. There is no section to render the alert into, so callers fall
    /// back to a standalone error card.
    BadRoot,
    /// In-root write failure. The store fetched the still-valid view on the
    /// failure path so the caller can render an inline alert by the affected
    /// row without a second store hop.
    Failed {
        /// The underlying domain error that caused the write to fail.
        error: WriteError,
        /// The still-valid raw view, fetched by the store on the failure path.
        raw: Arc<RawView>,
    },
}

impl RawViewStore {
    /// Write a marker into a folder and update the cached raw view in place,
    /// without a rescan (ADR-0002). The guard and write run on a blocking task.
    /// The cache lock is held only for the in-memory mutation.
    pub(crate) async fn write_mark(
        &self,
        root: usize,
        rel: &str,
        marker: Marker,
    ) -> Result<Applied, WriteFailure> {
        let root_path = self
            .config
            .library_roots
            .get(root)
            .ok_or(WriteFailure::BadRoot)?
            .clone();
        let rel_owned = rel.to_string();
        let inner =
            tokio::task::spawn_blocking(move || write_marker(&root_path, &rel_owned, marker))
                .await
                .map_err(|_| {
                    WriteError::WriteFailed(std::io::Error::other("marker write task failed"))
                })
                .and_then(|inner| inner);
        let (created, canonical_root) = match inner {
            Ok(pair) => pair,
            Err(error) => {
                // The write failed. Hand back the still-valid view so the
                // caller renders an inline alert without a second store hop.
                let raw = self.current().await;
                return Err(WriteFailure::Failed { error, raw });
            }
        };

        // A self-write may not bump the folder mtime, so force a re-list on rescan.
        self.invalidate_index(root, &canonical_root, rel);

        let rel_for_edit = rel.to_string();
        let raw = {
            // The inflight lock doubles as the edit-coordination lock: it
            // serializes a warm in-place edit against any concurrent cold
            // build's store, and against another concurrent edit.
            let _edit = self.inflight.lock().await;
            match self.cache.load_full() {
                Some(entry) if is_fresh(&entry, self.ttl) => {
                    let mut next = (*entry.raw).clone();
                    apply_mark_raw(&mut next, root, &rel_for_edit, marker);
                    let next = Arc::new(next);
                    // Preserve `stored_at`: an in-place edit does not refresh
                    // the freshness clock.
                    self.cache.store(Some(Arc::new(CacheEntry {
                        stored_at: entry.stored_at,
                        raw: Arc::clone(&next),
                    })));
                    next
                }
                _ => self.store_fresh(Arc::new(self.build_view().await)),
            }
        };
        Ok(Applied { raw, created })
    }

    /// Drop the index entry for `rel` under `canonical_root` so the next walk
    /// re-lists it. Used after this process writes or deletes a marker, so
    /// the change is picked up even if the directory mtime resolution would
    /// have hidden the same-tick write. A no-op when the path was never
    /// indexed, or when `root` is out of range.
    ///
    /// `canonical_root` must already be canonicalized by the caller.
    /// `write_marker` and `delete_marker` do that on `spawn_blocking` and
    /// hand back the result, so the sync `canonicalize` syscall stays off
    /// the async runtime thread.
    fn invalidate_index(&self, root: usize, canonical_root: &Path, rel: &str) {
        let Some(index) = self.dir_indices.get(root) else {
            return;
        };
        let target = if rel == "." {
            canonical_root.to_path_buf()
        } else {
            canonical_root.join(rel)
        };
        index.invalidate(&target);
    }

    /// Delete a marker file and refresh the cached view by rescanning the one
    /// affected root (ADR-0002). The guard and delete run on a blocking task.
    /// The cache lock is held only for the per-root rebuild.
    pub(crate) async fn remove_mark(
        &self,
        root: usize,
        rel: &str,
        marker: Marker,
    ) -> Result<Arc<RawView>, WriteFailure> {
        let root_path = self
            .config
            .library_roots
            .get(root)
            .ok_or(WriteFailure::BadRoot)?
            .clone();
        let rel_owned = rel.to_string();
        let delete_path = root_path.clone();
        let inner =
            tokio::task::spawn_blocking(move || delete_marker(&delete_path, &rel_owned, marker))
                .await
                .map_err(|_| {
                    WriteError::WriteFailed(std::io::Error::other("marker delete task failed"))
                })
                .and_then(|inner| inner);
        let canonical_root = match inner {
            Ok(path) => path,
            Err(error) => {
                let raw = self.current().await;
                return Err(WriteFailure::Failed { error, raw });
            }
        };

        // A self-delete may not bump the folder mtime, so force a re-list.
        self.invalidate_index(root, &canonical_root, rel);

        // Walk the one affected root BEFORE taking the lock. The per-root walk
        // can be slow on a network mount, and ADR-0027 keeps the inflight lock
        // held for microseconds only. The dir index is warm (we just
        // invalidated one entry), so this is the cheap re-list of that subtree.
        let section = build_section(
            root_path.clone(),
            Arc::clone(&self.settings),
            Arc::clone(&self.dir_indices[root]),
        )
        .await;

        // Splice the freshly walked section into the cached view, holding the
        // lock only for the load-clone-splice-store. The lock doubles as the
        // edit-coordination lock against a concurrent cold build's store and
        // another concurrent edit.
        let spliced = {
            let _edit = self.inflight.lock().await;
            match self.cache.load_full() {
                Some(entry) if is_fresh(&entry, self.ttl) => {
                    let mut next = (*entry.raw).clone();
                    if root < next.len() {
                        next[root] = section;
                    }
                    let next = Arc::new(next);
                    self.cache.store(Some(Arc::new(CacheEntry {
                        stored_at: entry.stored_at,
                        raw: Arc::clone(&next),
                    })));
                    Some(next)
                }
                _ => None,
            }
        };
        match spliced {
            Some(raw) => Ok(raw),
            // Cold or stale slot: rebuild through the single-flight coalescer
            // with no lock held across the walk. recheck_fresh is false because
            // the slot was just observed not-fresh and we want the rebuild.
            None => Ok(self.build_coalesced(false).await),
        }
    }
}

/// Guard the target and create the marker file, on a blocking task. The root
/// base comes from config, so only `rel` is request-controlled. It is
/// re-validated by canonicalizing the join and confirming it stays inside the
/// root. The open is create-only: `Ok(true)` when this call made the file,
/// `Ok(false)` when it was already there, which keeps a re-mark a no-op and
/// lets undo delete only files it created.
fn write_marker(root: &Path, rel: &str, marker: Marker) -> Result<(bool, PathBuf), WriteError> {
    let started = Instant::now();
    let canonical_root = std::fs::canonicalize(root).map_err(|_| WriteError::TargetMissing)?;
    let target = if rel == "." {
        canonical_root.clone()
    } else {
        canonical_root.join(rel)
    };
    let canonical_target = std::fs::canonicalize(&target).map_err(|_| WriteError::TargetMissing)?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(WriteError::OutsideRoots);
    }
    if !canonical_target.is_dir() {
        return Err(WriteError::NotADirectory);
    }
    let created = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(canonical_target.join(marker.filename()))
    {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(WriteError::WriteFailed(e)),
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
fn delete_marker(root: &Path, rel: &str, marker: Marker) -> Result<PathBuf, WriteError> {
    let started = Instant::now();
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(root.to_path_buf()),
        Err(_) => return Err(WriteError::TargetMissing),
    };
    let target = if rel == "." {
        canonical_root.clone()
    } else {
        canonical_root.join(rel)
    };
    let canonical_target = match std::fs::canonicalize(&target) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(canonical_root),
        Err(_) => return Err(WriteError::TargetMissing),
    };
    if !canonical_target.starts_with(&canonical_root) {
        return Err(WriteError::OutsideRoots);
    }
    let removed = match std::fs::remove_file(canonical_target.join(marker.filename())) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(WriteError::WriteFailed(e)),
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
        let dir_indices = vec![Arc::new(DirIndex::new())];
        RawViewStore::new(Arc::new(cfg), Arc::new(settings), dir_indices, ttl)
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
        let after_warm = store.dir_indices[0].len();
        assert!(after_warm > 0, "the warm read populated the dir index");

        // Drop a synthetic entry into the index that no real walk could reach.
        // A cold rescan must drop it. A warm rescan would preserve it.
        let synthetic_path = std::path::PathBuf::from("/nonexistent/synthetic/marker/path");
        store.dir_indices[0].insert(
            synthetic_path.clone(),
            scanner::CachedDir {
                mtime: std::time::UNIX_EPOCH,
                subdirs: std::sync::Arc::from(Vec::<std::path::PathBuf>::new()),
                cover_files: std::sync::Arc::from(Vec::<String>::new()),
                audio_files: std::sync::Arc::from(Vec::<String>::new()),
            },
        );
        assert!(
            store.dir_indices[0].get_cloned(&synthetic_path).is_some(),
            "synthetic entry is present before rescan"
        );
        // Rescan must drop every entry, then the rebuild repopulates it.
        let _ = store.rescan().await;
        assert!(
            store.dir_indices[0].get_cloned(&synthetic_path).is_none(),
            "rescan must clear the dir index, dropping the synthetic entry"
        );
        let after_rescan = store.dir_indices[0].len();
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
        assert!(matches!(err, WriteFailure::BadRoot));
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
        assert!(matches!(err, WriteError::OutsideRoots));
    }

    #[test]
    fn write_marker_missing_target_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_marker(dir.path(), "Nope", Marker::NoEbook).unwrap_err();
        assert!(matches!(err, WriteError::TargetMissing));
    }

    #[test]
    fn write_marker_rejects_a_file_target() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let err = write_marker(dir.path(), "Book/01.mp3", Marker::NoEbook).unwrap_err();
        assert!(matches!(err, WriteError::NotADirectory));
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
        assert!(matches!(err, WriteError::OutsideRoots));
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

    #[tokio::test]
    async fn store_remove_mark_fresh_path_splices_without_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let _warm = store.current().await;
        let _ = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        let before = store.rebuild_count();

        let after = store.remove_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert_eq!(
            store.rebuild_count(),
            before,
            "a warm undo splices in place; it must not rebuild the slot",
        );
        let scanner::RootScan::Walked { folders, .. } = &after[0] else {
            panic!("expected Walked");
        };
        let book = folders
            .iter()
            .find(|f| f.rel_path.to_str() == Some("Book"))
            .unwrap();
        assert!(book.missing_ebook, "the undo re-flagged the folder");
    }

    #[tokio::test]
    async fn store_remove_mark_cold_slot_rebuilds_via_coalesced() {
        // ttl 0 means the slot is never fresh, so the splice arm is skipped and
        // the cold fallback rebuilds through build_coalesced.
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(None, dir.path().to_path_buf());

        // Create the marker on disk, then undo it. The undo's fallback build
        // must produce a view with the folder re-flagged.
        let _ = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        let before = store.rebuild_count();
        let after = store.remove_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(
            store.rebuild_count() > before,
            "a cold undo must rebuild through build_coalesced",
        );
        let scanner::RootScan::Walked { folders, .. } = &after[0] else {
            panic!("expected Walked");
        };
        assert!(
            folders.iter().any(|f| f.missing_ebook),
            "the rebuilt view shows the re-flagged gap",
        );
    }

    #[tokio::test]
    async fn store_concurrent_read_coexists_with_an_undo() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = Arc::new(test_store(
            Some(Duration::from_secs(600)),
            dir.path().to_path_buf(),
        ));

        let _warm = store.current().await;
        let _ = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        let before = store.rebuild_count();

        let s_undo = Arc::clone(&store);
        let s_read = Arc::clone(&store);
        let (undo, read) = tokio::join!(
            s_undo.remove_mark(0, "Book", Marker::NoEbook),
            s_read.current(),
        );
        undo.unwrap();
        assert_eq!(
            read.len(),
            1,
            "the concurrent read returns one section per root"
        );
        assert_eq!(
            store.rebuild_count(),
            before,
            "a warm read concurrent with a warm undo must not rebuild",
        );
    }

    #[tokio::test]
    async fn build_coalesced_recheck_returns_fresh_without_rebuild() {
        // The current() path passes recheck_fresh=true: a fresh slot observed
        // under the lock is returned without a redundant second walk.
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let _warm = store.current().await;
        let before = store.rebuild_count();
        let _ = store.build_coalesced(true).await;
        assert_eq!(
            store.rebuild_count(),
            before,
            "recheck_fresh must short-circuit on a fresh slot, not rebuild",
        );
    }

    #[tokio::test]
    async fn build_coalesced_without_recheck_rebuilds_even_when_fresh() {
        // refresh()/rescan() pass recheck_fresh=false so they pick up disk
        // changes regardless of a live TTL: a fresh slot must not short-circuit.
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let _warm = store.current().await;
        let before = store.rebuild_count();
        let _ = store.build_coalesced(false).await;
        assert_eq!(
            store.rebuild_count(),
            before + 1,
            "a forced build must rebuild even when the slot is fresh",
        );
    }

    #[tokio::test]
    async fn store_scans_independent_roots_without_cross_root_lock_contention() {
        // Two roots, each with one gap. A single shared index would serialize
        // the walks. Per-root indices let them proceed independently. We
        // assert the weaker, deterministic property: both roots scan and both
        // gaps surface.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&a.path().join("Book/01.mp3"));
        crate::scenarios::touch(&b.path().join("Book/01.mp3"));
        let cfg = Config {
            library_roots: vec![a.path().to_path_buf(), b.path().to_path_buf()],
            ttl_seconds: 600,
            ..Default::default()
        };
        let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
        let dir_indices = (0..2).map(|_| Arc::new(DirIndex::new())).collect();
        let store = RawViewStore::new(
            Arc::new(cfg),
            Arc::new(settings),
            dir_indices,
            Some(Duration::from_secs(600)),
        );
        let raw = store.current().await;
        assert_eq!(raw.len(), 2, "one section per root");
        for section in raw.iter() {
            let RootScan::Walked { folders, .. } = section else {
                panic!("expected Walked");
            };
            assert!(
                folders.iter().any(|f| f.missing_ebook),
                "each root has a gap"
            );
        }
    }

    /// Helper: assert that `Book` under root 0 of `raw` has the expected
    /// `missing_ebook` value. Used by ported tests that previously asserted on
    /// the packaged `RootState`. The equivalent at the raw layer reads off the
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
        // index keys off mtime equality, so back-to-back touches inside one tick
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

        let reused = store.dir_indices[0].len();
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
            store.dir_indices[0].get_cloned(&book).is_some(),
            "Book is indexed after the scan"
        );

        // Marking Book writes .no_ebook into it, so its index entry must be dropped
        // and the next walk re-lists it rather than trusting a pre-write mtime.
        store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(
            store.dir_indices[0].get_cloned(&book).is_none(),
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
        assert!(matches!(
            err,
            WriteFailure::Failed {
                error: WriteError::OutsideRoots,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn store_remove_mark_bad_root_index_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let err = store
            .remove_mark(9, ".", Marker::NoEbook)
            .await
            .unwrap_err();
        assert!(matches!(err, WriteFailure::BadRoot));
    }

    #[tokio::test]
    async fn store_write_mark_failure_returns_current_raw_view() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let _warm = store.current().await;
        let rebuilds_before = store.rebuild_count();

        let err = store
            .write_mark(0, "..", Marker::NoEbook)
            .await
            .unwrap_err();
        let raw = match err {
            WriteFailure::Failed {
                error: WriteError::OutsideRoots,
                raw,
            } => raw,
            other => panic!("expected Failed with OutsideRoots, got {other:?}"),
        };

        assert_eq!(
            store.rebuild_count(),
            rebuilds_before,
            "warm failure path must not rebuild",
        );
        assert_eq!(raw.len(), 1, "raw carries one section per library root");
    }

    #[tokio::test]
    async fn store_write_mark_failure_on_cold_cache_warms_the_slot() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        let err = store
            .write_mark(0, "..", Marker::NoEbook)
            .await
            .unwrap_err();
        let raw = match err {
            WriteFailure::Failed {
                error: WriteError::OutsideRoots,
                raw,
            } => raw,
            other => panic!("expected Failed with OutsideRoots, got {other:?}"),
        };

        assert_eq!(
            store.rebuild_count(),
            1,
            "cold-cache failure path builds once",
        );
        assert_eq!(raw.len(), 1, "freshly built view has one section per root");

        let _follow_up = store.current().await;
        assert_eq!(
            store.rebuild_count(),
            1,
            "follow-up current() reuses the warmed slot",
        );
    }
}
