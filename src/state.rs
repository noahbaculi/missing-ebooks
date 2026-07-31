//! Application state: the immutable `Arc<Config>` and compiled `Arc<ScanSettings>`,
//! plus a TTL-memoized scan cache behind one mutex. The cache stores the raw
//! per-root walk output and the response renders per `ViewMode` on each read
//! (see ADR-0022). A marker write updates the stored raw view in place rather
//! than rewalking (see docs/adr/0002-marker-writes-edit-cache-in-place.md).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use tokio::sync::Mutex;

use crate::config::Config;
use crate::marker::Marker;
use crate::raw_view::{RawView, apply_mark_raw, build_section, build_view};
use crate::scanner::{DirIndex, ScanSettings};

/// Everything a request handler needs: the immutable config and settings, and
/// the scan cache. Shared as `Arc<AppState>`.
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) store: RawViewStore,
}

/// A stored raw view and the instant it was scanned. Owned by `RawViewStore`'s
/// cache slot.
struct CacheEntry {
    stored_at: Instant,
    raw: Arc<RawView>,
}

/// The guarded cache cell: the entry plus a monotonically increasing
/// generation. Every store and every in-place edit bumps the generation, so
/// a build that began before the latest write detects the move and declines
/// to overwrite it (newest write wins, ADR-0036)
struct Slot {
    generation: u64,
    entry: Option<Arc<CacheEntry>>,
}

/// Test-only pause point between a cold build's walk and its store, so
/// interleaving tests can act while the result is in hand but unpublished
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct BuildGate {
    /// The build adds one permit here when it reaches the gate
    pub(crate) entered: Arc<tokio::sync::Semaphore>,
    /// The build consumes one permit from here before storing
    pub(crate) release: Arc<tokio::sync::Semaphore>,
}

#[cfg(test)]
impl BuildGate {
    pub(crate) fn new() -> BuildGate {
        BuildGate {
            entered: Arc::new(tokio::sync::Semaphore::new(0)),
            release: Arc::new(tokio::sync::Semaphore::new(0)),
        }
    }
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
    /// Shared internals, behind one `Arc` so mutation sequences can run in
    /// spawned tasks that outlive a dropped request handler
    inner: Arc<StoreInner>,
}

/// The store's fields and lock protocol. `RawViewStore` methods delegate
/// here; spawned tasks capture an `Arc<StoreInner>` clone
struct StoreInner {
    /// The cache slot. Reads clone the `Arc<CacheEntry>` out under the read
    /// lock. At self-hosted request rates an uncontended read costs about as
    /// much as an atomic pointer load, and the in-place edit paths are
    /// atomic under the write lock.
    cache: std::sync::RwLock<Slot>,
    /// Holds only the in-flight build handle (as a `Weak`, so it expires
    /// when the last awaiter drops it). Held for microseconds to register
    /// or join a build, never across the walk. Also serializes the
    /// load-edit-store of marker edits against each other. Ordering against
    /// a concurrent build's store is the slot generation's job, not this
    /// lock's (see ADR-0036)
    inflight: Mutex<Option<Weak<SharedBuild>>>,
    /// Test-only pause point armed by `set_build_gate`, checked by every cold
    /// build between its walk and its store.
    #[cfg(test)]
    build_gate: std::sync::Mutex<Option<BuildGate>>,
    /// Monotonic count of fresh builds stored into the slot. Bumped by
    /// `store_if_unchanged`. Tests diff before vs. after to assert that a
    /// warm operation did not rebuild. See ADR-0022.
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
            inner: Arc::new(StoreInner {
                cache: std::sync::RwLock::new(Slot {
                    generation: 0,
                    entry: None,
                }),
                inflight: Mutex::new(None),
                #[cfg(test)]
                build_gate: std::sync::Mutex::new(None),
                rebuild_count: AtomicU64::new(0),
                ttl,
                settings,
                dir_indices,
                config,
            }),
        }
    }

    /// Return the cached raw view if still fresh, otherwise build one. Reads
    /// take an uncontended read lock. Concurrent cold reads coalesce onto one build.
    /// TTL-respecting. Used by page loads and the `/refresh` poll handler.
    pub async fn current(&self) -> Arc<RawView> {
        StoreInner::current(&self.inner).await
    }

    /// Force a cold scan: clear every per-root index, then rebuild. Ignores
    /// the TTL. Spawned so an aborted request cannot strand cleared indices
    /// with nothing to refill them
    pub(crate) async fn rescan(&self) -> Arc<RawView> {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            for index in &inner.dir_indices {
                index.clear();
            }
            StoreInner::build_coalesced(&inner, false).await
        })
        .await
        .expect("rescan task panicked")
    }

    /// Returns the count of fresh builds stored into the slot since this
    /// store was created.
    #[cfg(test)]
    pub fn rebuild_count(&self) -> u64 {
        self.inner.rebuild_count.load(Ordering::Relaxed)
    }

    /// Test-only seam onto one root's persistent `DirIndex` entry count, so
    /// tests can assert that a build path did not clear the per-root mtime
    /// cache.
    #[cfg(test)]
    pub(crate) fn dir_index_len_for_test(&self, root: usize) -> usize {
        self.inner.dir_indices[root].len()
    }

    /// Arm the test-only build gate. Every later cold build pauses at it
    #[cfg(test)]
    pub(crate) fn set_build_gate(&self, gate: BuildGate) {
        *self
            .inner
            .build_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate);
    }

    /// Test-only read of the stored raw slot, ignoring freshness
    #[cfg(test)]
    fn peek_stored_arc(&self) -> Option<Arc<RawView>> {
        self.inner.load().map(|entry| Arc::clone(&entry.raw))
    }
}

impl StoreInner {
    /// Read the cache slot, recovering the guard on poison. A poisoned slot
    /// still holds a coherent entry (writers replace the whole `Option` in
    /// one assignment), so recovery beats wedging every later read.
    fn load(&self) -> Option<Arc<CacheEntry>> {
        self.cache
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry
            .clone()
    }

    /// Write-lock the cache slot, recovering the guard on poison for the
    /// same reason `load` does.
    fn slot(&self) -> std::sync::RwLockWriteGuard<'_, Slot> {
        self.cache
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Current slot generation. Read under the inflight lock at build
    /// registration so the pairing with `store_if_unchanged` is race-free
    fn generation(&self) -> u64 {
        self.cache
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    /// Store a freshly built raw view unless the slot moved since `expected`
    /// was read. A mark or undo that landed mid-walk already bumped the
    /// generation, and newer data must not be overwritten by an older walk.
    /// Bumps `rebuild_count` only when the store happens
    fn store_if_unchanged(&self, expected: u64, raw: &Arc<RawView>) {
        let mut slot = self.slot();
        if slot.generation != expected {
            tracing::debug!("discarding a stale build; the slot moved during the walk");
            return;
        }
        slot.generation += 1;
        slot.entry = Some(Arc::new(CacheEntry {
            stored_at: Instant::now(),
            raw: Arc::clone(raw),
        }));
        self.rebuild_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Clone-edit-store the fresh entry under the write lock, preserving
    /// `stored_at` and bumping the generation. `None` when the slot is cold or
    /// stale, leaving the caller to rebuild
    fn edit_fresh(&self, edit: impl FnOnce(&mut RawView)) -> Option<Arc<RawView>> {
        let mut slot = self.slot();
        let entry = slot.entry.as_ref()?;
        if !is_fresh(entry, self.ttl) {
            return None;
        }
        let stored_at = entry.stored_at;
        let mut next = (*entry.raw).clone();
        edit(&mut next);
        let next = Arc::new(next);
        slot.generation += 1;
        slot.entry = Some(Arc::new(CacheEntry {
            stored_at,
            raw: Arc::clone(&next),
        }));
        Some(next)
    }

    /// Block at the armed build gate, if any. Compiled out of production
    #[cfg(test)]
    async fn pause_at_build_gate(&self) {
        let gate = self
            .build_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            gate.release
                .acquire()
                .await
                .expect("build gate semaphore closed")
                .forget();
        }
    }

    /// Return the cached raw view if still fresh, otherwise build one. Reads
    /// take an uncontended read lock. Concurrent cold reads coalesce onto one build.
    /// TTL-respecting. Used by page loads and the `/refresh` poll handler.
    async fn current(this: &Arc<StoreInner>) -> Arc<RawView> {
        if let Some(entry) = this.load()
            && is_fresh(&entry, this.ttl)
        {
            tracing::debug!("cache hit");
            return Arc::clone(&entry.raw);
        }
        tracing::debug!("cache miss");
        StoreInner::build_coalesced(this, true).await
    }

    /// Start a build, or join the one already running. The walk and its store
    /// run in a spawned task, so a dropped request cannot discard a completed
    /// build. The store is generation-guarded: registered under the inflight
    /// lock, compared at store time, skipped when an edit landed mid-walk
    ///
    /// `recheck_fresh`: when true (the `current` read path), re-check the cache
    /// under the lock and return a fresh entry instead of starting a new walk.
    /// This closes the cold-burst race where the owning build stored a fresh
    /// entry and dropped its in-flight handle while this caller waited for the
    /// lock, which would otherwise trigger a redundant second full walk. The
    /// `refresh`/`rescan`/edit-fallback callers pass false: they must rebuild
    /// regardless of a live TTL.
    async fn build_coalesced(this: &Arc<StoreInner>, recheck_fresh: bool) -> Arc<RawView> {
        let handle = {
            let mut slot = this.inflight.lock().await;
            if recheck_fresh
                && let Some(entry) = this.load()
                && is_fresh(&entry, this.ttl)
            {
                return Arc::clone(&entry.raw);
            }
            if let Some(existing) = slot.as_ref().and_then(Weak::upgrade) {
                existing
            } else {
                let registered_at = this.generation();
                let task_inner = Arc::clone(this);
                let task = tokio::spawn(async move {
                    let raw = Arc::new(
                        build_view(
                            task_inner.config.as_ref(),
                            &task_inner.settings,
                            &task_inner.dir_indices,
                        )
                        .await,
                    );
                    #[cfg(test)]
                    task_inner.pause_at_build_gate().await;
                    task_inner.store_if_unchanged(registered_at, &raw);
                    raw
                });
                let build: SharedBuild = async move {
                    // JoinError is unreachable in practice: per-root panics are
                    // folded into RootScan::Failed by build_section
                    task.await.expect("cold build task panicked")
                }
                .boxed()
                .shared();
                let handle = Arc::new(build);
                *slot = Some(Arc::downgrade(&handle));
                handle
            }
        };
        // Keep the Arc alive across the await so a concurrent caller can
        // upgrade the Weak and join this build
        (*handle).clone().await
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
        // Path equality and hashing normalize a trailing ".", so rel "." hits the root entry
        index.invalidate(&canonical_root.join(rel));
    }

    /// The whole mark sequence: guard-and-write on the blocking pool, index
    /// invalidation, cache edit or coalesced rebuild. Runs inside a spawned
    /// task so a dropped handler cannot split the write from its bookkeeping
    async fn apply_write_mark(
        this: &Arc<StoreInner>,
        root_path: PathBuf,
        root: usize,
        rel: &str,
        marker: Marker,
    ) -> Result<Applied, WriteFailure> {
        let rel_owned = rel.to_string();
        let io = tokio::task::spawn_blocking(move || write_marker(&root_path, &rel_owned, marker))
            .await
            .map_err(|_| WriteError::WriteFailed(std::io::Error::other("marker write task failed")))
            .and_then(|io| io);
        let (created, canonical_root) = match io {
            Ok(pair) => pair,
            Err(error) => {
                // The write failed. Hand back the still-valid view so the
                // caller renders an inline alert without a second store hop
                let raw = StoreInner::current(this).await;
                return Err(WriteFailure::Failed { error, raw });
            }
        };
        // A re-mark created nothing on disk: no index entry went stale and no
        // cover-file list changed, so skip the clone-and-edit entirely (F12)
        if !created {
            return Ok(Applied {
                raw: StoreInner::current(this).await,
                created,
            });
        }
        // A self-write may not bump the folder mtime, so force a re-list
        this.invalidate_index(root, &canonical_root, rel);

        // Mirror remove_mark: the inflight guard scopes to the load-edit-store
        // only, never across a walk (F1)
        let edited = {
            let _edit = this.inflight.lock().await;
            this.edit_fresh(|next| apply_mark_raw(next, root, rel, marker))
        };
        let raw = match edited {
            Some(next) => next,
            // Cold or stale slot: the marker is already on disk, so a coalesced
            // rebuild reflects it and concurrent cold marks share one walk
            None => StoreInner::build_coalesced(this, false).await,
        };
        Ok(Applied { raw, created })
    }

    /// The whole undo sequence: guard-and-delete on the blocking pool, index
    /// invalidation, per-root rewalk, splice or coalesced rebuild. Spawned for
    /// the same reason as apply_write_mark
    async fn apply_remove_mark(
        this: &Arc<StoreInner>,
        root_path: PathBuf,
        root: usize,
        rel: &str,
        marker: Marker,
    ) -> Result<Arc<RawView>, WriteFailure> {
        let rel_owned = rel.to_string();
        let delete_path = root_path.clone();
        let io =
            tokio::task::spawn_blocking(move || delete_marker(&delete_path, &rel_owned, marker))
                .await
                .map_err(|_| {
                    WriteError::WriteFailed(std::io::Error::other("marker delete task failed"))
                })
                .and_then(|io| io);
        let canonical_root = match io {
            Ok(path) => path,
            Err(error) => {
                let raw = StoreInner::current(this).await;
                return Err(WriteFailure::Failed { error, raw });
            }
        };

        // A self-delete may not bump the folder mtime, so force a re-list
        this.invalidate_index(root, &canonical_root, rel);

        // Walk the one affected root BEFORE taking the lock. ADR-0027 keeps
        // the inflight lock held for microseconds only, and the dir index is
        // warm, so this is the cheap re-list of that subtree
        let section = build_section(
            root_path,
            Arc::clone(&this.settings),
            Arc::clone(&this.dir_indices[root]),
        )
        .await;

        let spliced = {
            let _edit = this.inflight.lock().await;
            this.edit_fresh(|next| {
                if root < next.len() {
                    next[root] = section;
                }
            })
        };
        match spliced {
            Some(raw) => Ok(raw),
            // Cold or stale slot: rebuild through the single-flight coalescer
            // with no lock held across the walk
            None => Ok(StoreInner::build_coalesced(this, false).await),
        }
    }

    /// Spawn a mutation's sequence and recover a task panic into the same
    /// WriteFailure shape a normal write failure produces, so callers don't
    /// see a spawn as a different error surface than an IO failure
    async fn spawn_mutation<T: Send + 'static>(
        inner: &Arc<StoreInner>,
        op: &'static str,
        fut: impl Future<Output = Result<T, WriteFailure>> + Send + 'static,
    ) -> Result<T, WriteFailure> {
        match tokio::spawn(fut).await {
            Ok(result) => result,
            Err(join_err) => {
                tracing::error!(error = %join_err, "{op} task panicked");
                let raw = StoreInner::current(inner).await;
                Err(WriteFailure::Failed {
                    error: WriteError::WriteFailed(std::io::Error::other(format!(
                        "{op} task failed"
                    ))),
                    raw,
                })
            }
        }
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
        let dir_indices = (0..config.library_roots.len())
            .map(|_| Arc::new(DirIndex::new()))
            .collect();
        let store = RawViewStore::new(Arc::clone(&config), Arc::new(settings), dir_indices, ttl);
        AppState { config, store }
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
            .inner
            .config
            .library_roots
            .get(root)
            .ok_or(WriteFailure::BadRoot)?
            .clone();
        let inner = Arc::clone(&self.inner);
        let rel = rel.to_string();
        // Spawned so a client disconnect cannot split the marker write from
        // its index invalidation and cache edit (ADR-0036)
        StoreInner::spawn_mutation(&self.inner, "mark", async move {
            StoreInner::apply_write_mark(&inner, root_path, root, &rel, marker).await
        })
        .await
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
            .inner
            .config
            .library_roots
            .get(root)
            .ok_or(WriteFailure::BadRoot)?
            .clone();
        let inner = Arc::clone(&self.inner);
        let rel = rel.to_string();
        StoreInner::spawn_mutation(&self.inner, "unmark", async move {
            StoreInner::apply_remove_mark(&inner, root_path, root, &rel, marker).await
        })
        .await
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
    let target = canonical_root.join(rel);
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
    let target = canonical_root.join(rel);
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
        assert_eq!(state.store.inner.ttl, None);
    }

    #[test]
    fn nonzero_ttl_sets_the_window() {
        let cfg = Config {
            ttl_seconds: 90,
            ..Default::default()
        };
        let state = AppState::new(cfg, settings());
        assert_eq!(state.store.inner.ttl, Some(Duration::from_secs(90)));
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
        let after_warm = store.inner.dir_indices[0].len();
        assert!(after_warm > 0, "the warm read populated the dir index");

        // Drop a synthetic entry into the index that no real walk could reach.
        // A cold rescan must drop it. A warm rescan would preserve it.
        let synthetic_path = std::path::PathBuf::from("/nonexistent/synthetic/marker/path");
        store.inner.dir_indices[0].insert(
            synthetic_path.clone(),
            scanner::CachedDir {
                mtime: std::time::UNIX_EPOCH,
                subdirs: std::sync::Arc::from(Vec::<std::path::PathBuf>::new()),
                cover_files: std::sync::Arc::from(Vec::<String>::new()),
                audio_files: std::sync::Arc::from(Vec::<String>::new()),
            },
        );
        assert!(
            store.inner.dir_indices[0]
                .get_cloned(&synthetic_path)
                .is_some(),
            "synthetic entry is present before rescan"
        );
        // Rescan must drop every entry, then the rebuild repopulates it.
        let _ = store.rescan().await;
        assert!(
            store.inner.dir_indices[0]
                .get_cloned(&synthetic_path)
                .is_none(),
            "rescan must clear the dir index, dropping the synthetic entry"
        );
        let after_rescan = store.inner.dir_indices[0].len();
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
    fn canonicalize_resolves_root_join_dot_to_the_root() {
        // Pins the invariant the rel == "." special case in write_marker and
        // delete_marker relied on: joining "." and canonicalizing lands on
        // the canonical root, so the join needs no special-casing.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            std::fs::canonicalize(dir.path().join(".")).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
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
    async fn a_dropped_mark_still_lands_its_bookkeeping() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let _warm = store.current().await;
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert!(
            store.inner.dir_indices[0]
                .get_cloned(&canonical.join("Book"))
                .is_some()
        );

        {
            // One poll starts the spawned task, then the handler-side future
            // drops, standing in for a client disconnect
            let fut = store.write_mark(0, "Book", Marker::NoEbook);
            let mut fut = std::pin::pin!(fut);
            let _ = tokio::time::timeout(Duration::from_millis(0), fut.as_mut()).await;
        }

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let marker_on_disk = dir.path().join("Book/.no_ebook").exists();
                let index_invalidated = store.inner.dir_indices[0]
                    .get_cloned(&canonical.join("Book"))
                    .is_none();
                let view_edited = store
                    .peek_stored_arc()
                    .is_some_and(|raw| !book_missing(&raw));
                if marker_on_disk && index_invalidated && view_edited {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the spawned mark task never finished its bookkeeping");
    }

    #[tokio::test]
    async fn a_dropped_unmark_still_lands_its_bookkeeping() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let _warm = store.current().await;
        let _ = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();

        {
            let fut = store.remove_mark(0, "Book", Marker::NoEbook);
            let mut fut = std::pin::pin!(fut);
            let _ = tokio::time::timeout(Duration::from_millis(0), fut.as_mut()).await;
        }

        // The splice re-lists Book into the index, so assert on the two ends:
        // the file is gone and the stored view re-flags the folder
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let marker_gone = !dir.path().join("Book/.no_ebook").exists();
                let view_reflagged = store
                    .peek_stored_arc()
                    .is_some_and(|raw| book_missing(&raw));
                if marker_gone && view_reflagged {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the spawned unmark task never finished its bookkeeping");
    }

    #[tokio::test]
    async fn a_dropped_rescan_still_repopulates_the_index() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let _warm = store.current().await;
        let before = store.rebuild_count();

        {
            let fut = store.rescan();
            let mut fut = std::pin::pin!(fut);
            let _ = tokio::time::timeout(Duration::from_millis(0), fut.as_mut()).await;
        }

        tokio::time::timeout(Duration::from_secs(5), async {
            while store.rebuild_count() == before {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the spawned rescan task never rebuilt");
        assert!(
            store.inner.dir_indices[0].len() > 0,
            "the aborted rescan left a repopulated index, not a stranded empty one"
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
        let _ = StoreInner::build_coalesced(&store.inner, true).await;
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
        let _ = StoreInner::build_coalesced(&store.inner, false).await;
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
        std::fs::File::open(&book)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(4_000_000_000))
            .unwrap();
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

        let reused = store.inner.dir_indices[0].len();
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
            store.inner.dir_indices[0].get_cloned(&book).is_some(),
            "Book is indexed after the scan"
        );

        // Marking Book writes .no_ebook into it, so its index entry must be dropped
        // and the next walk re-lists it rather than trusting a pre-write mtime.
        store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(
            store.inner.dir_indices[0].get_cloned(&book).is_none(),
            "Book's index entry is invalidated by the marker write"
        );
    }

    #[tokio::test]
    async fn store_write_mark_at_the_root_invalidates_the_root_in_the_index() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());

        // Warm the index by scanning once.
        store.current().await;
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert!(
            store.inner.dir_indices[0].get_cloned(&canonical).is_some(),
            "the root is indexed after the scan"
        );

        // Marking the root flows rel "." into invalidate_index, whose join
        // yields "root/.". Path equality and hashing normalize that back to
        // the root's index key.
        store.write_mark(0, ".", Marker::NoEbook).await.unwrap();
        assert!(
            store.inner.dir_indices[0].get_cloned(&canonical).is_none(),
            "the root's index entry is invalidated by the marker write"
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
    async fn concurrent_cold_marks_coalesce_into_one_walk() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("A/01.mp3"));
        crate::scenarios::touch(&dir.path().join("B/01.mp3"));
        let store = Arc::new(test_store(
            Some(Duration::from_secs(600)),
            dir.path().to_path_buf(),
        ));
        let gate = BuildGate::new();
        store.set_build_gate(gate.clone());

        let s1 = Arc::clone(&store);
        let first = tokio::spawn(async move { s1.write_mark(0, "A", Marker::NoEbook).await });
        // The first cold mark's fallback build reaches the gate after its walk.
        // A timeout here is the old shape failing: the private build_view arm
        // never routes through the coalesced (gated) build
        tokio::time::timeout(Duration::from_secs(5), gate.entered.acquire())
            .await
            .expect("the cold mark never routed through the coalesced build")
            .unwrap()
            .forget();

        let s2 = Arc::clone(&store);
        let second = tokio::spawn(async move { s2.write_mark(0, "B", Marker::NoEbook).await });
        // Drive the second mark past its marker write and into the join
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("B/.no_ebook").exists() {
                tokio::task::yield_now().await;
            }
            for _ in 0..64 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the second mark never wrote its marker");

        gate.release.add_permits(1);
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert_eq!(
            store.rebuild_count(),
            1,
            "concurrent cold marks share one coalesced walk"
        );
    }

    #[tokio::test]
    async fn a_remark_returns_the_cached_view_without_cloning_the_slot() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let _warm = store.current().await;
        let first = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(first.created);
        let arc_before = store.peek_stored_arc().unwrap();
        let generation_before = store.inner.generation();

        let second = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(!second.created);
        assert!(
            Arc::ptr_eq(&arc_before, &store.peek_stored_arc().unwrap()),
            "a re-mark must not clone and restore the slot"
        );
        assert_eq!(
            store.inner.generation(),
            generation_before,
            "a re-mark must not bump the slot generation"
        );
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

    #[tokio::test]
    async fn a_mark_landing_mid_build_survives_the_builds_store() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let _warm = store.current().await;

        let gate = BuildGate::new();
        store.set_build_gate(gate.clone());

        // A forced rebuild pauses between its walk and its store
        let inner = Arc::clone(&store.inner);
        let build = tokio::spawn(async move { StoreInner::build_coalesced(&inner, false).await });
        gate.entered.acquire().await.unwrap().forget();

        // The mark lands while the walk's result is in hand but unstored
        let applied = store.write_mark(0, "Book", Marker::NoEbook).await.unwrap();
        assert!(applied.created);

        gate.release.add_permits(1);
        let built = build.await.unwrap();
        assert!(
            book_missing(&built),
            "awaiters still receive the pre-mark walk"
        );

        let served = store.current().await;
        assert!(
            !book_missing(&served),
            "the interleaved mark survives the build's store"
        );
    }

    #[tokio::test]
    async fn an_aborted_owner_still_stores_the_completed_build() {
        let dir = tempfile::tempdir().unwrap();
        crate::scenarios::touch(&dir.path().join("Book/01.mp3"));
        let store = test_store(Some(Duration::from_secs(600)), dir.path().to_path_buf());
        let gate = BuildGate::new();
        store.set_build_gate(gate.clone());

        let inner = Arc::clone(&store.inner);
        let owner = tokio::spawn(async move { StoreInner::build_coalesced(&inner, true).await });
        gate.entered.acquire().await.unwrap().forget();

        // The only awaiter vanishes mid-build
        owner.abort();
        let _ = owner.await;

        gate.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), async {
            while store.rebuild_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the detached build task never stored its result");

        let _ = store.current().await;
        assert_eq!(
            store.rebuild_count(),
            1,
            "the follow-up read serves the stored slot without a second walk"
        );
    }
}
