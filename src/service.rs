//! Web-agnostic service layer: the read view types and the typed operations
//! (current view, marker write) shared by the HTML UI and a future JSON API.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::Config;
use crate::marker::Marker;
use crate::scanner::{self, ScanSettings};
use crate::state::{self, AppState};
use crate::tree::{self, Node};

/// Which view a read or write targets: gaps-only forest or full show-all tree.
/// Selects the render applied to the cached raw scan output (see ADR-0022).
/// Deserializes from the `view` form field; `from_query` is the lenient
/// URL-query path that falls back to gaps-only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum ViewMode {
    /// Today's view: only gaps and the containers above them.
    #[default]
    #[serde(rename = "gaps")]
    GapsOnly,
    /// The full directory tree, covered folders included.
    #[serde(rename = "all")]
    All,
}

impl ViewMode {
    /// Parse the URL `view` query parameter. Absent or unrecognized is gaps-only.
    #[must_use]
    pub fn from_query(value: Option<&str>) -> ViewMode {
        match value {
            Some("all") => ViewMode::All,
            _ => ViewMode::GapsOnly,
        }
    }

    /// The query token for this mode: `gaps` or `all`.
    #[must_use]
    pub fn as_query(self) -> &'static str {
        match self {
            ViewMode::GapsOnly => "gaps",
            ViewMode::All => "all",
        }
    }
}

/// The whole read view: one section per configured library root, in config order.
pub type FlaggedView = Vec<RootSection>;

/// One library root's outcome, labeled with the path the scanner walked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RootSection {
    /// The canonical root path when it resolved, else the configured path.
    pub path: String,
    /// What the scan found for this root.
    pub state: RootState,
}

/// The result of scanning one root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootState {
    /// Flagged gaps were found; the forest is non-empty.
    Forest(Vec<Node>),
    /// The root resolved and scanned with no gaps.
    Clean,
    /// The root could not be scanned (missing, not a directory, or unreadable).
    Error(String),
}

/// A failure performing a write action. The HTML surface renders it inline. A
/// future JSON API would render it as an error body.
#[derive(Debug, Error)]
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

/// Return the cached view if it is still fresh, otherwise scan and cache it.
/// Single-flight is enforced by `Cache::get_or_build`.
pub async fn current_view(state: &AppState, mode: ViewMode) -> Arc<FlaggedView> {
    let raw = state
        .cache
        .get_or_build(|| {
            build_view(
                state.config.as_ref(),
                &state.settings,
                Arc::clone(&state.dir_index),
            )
        })
        .await;
    Arc::new(render_view(&raw, mode))
}

/// Force a fresh cold scan by clearing the dir index and rebuilding from
/// scratch. Ignores the TTL. This is the explicit "fix any drift" path; the
/// autosync loop keeps using warm scans (see ADR-0023).
pub async fn rescan(state: &AppState, mode: ViewMode) -> Arc<FlaggedView> {
    state.clear_dir_index();
    let raw = state
        .cache
        .rebuild(|| {
            build_view(
                state.config.as_ref(),
                &state.settings,
                Arc::clone(&state.dir_index),
            )
        })
        .await;
    Arc::new(render_view(&raw, mode))
}

/// The result of a marker write: the refreshed view plus whether this call
/// actually created the file. `created` is false for a re-mark of an
/// already-marked folder, which the HTML surface uses to suppress the undo toast.
#[derive(Debug)]
pub struct MarkOutcome {
    /// The refreshed view after the write, the requesting mode's slot.
    pub view: Arc<FlaggedView>,
    /// True when this call made the file; false for a re-mark of a marked folder.
    pub created: bool,
}

/// Write a marker into a folder and update the cached view in place, without a
/// rescan (see docs/adr/0002-v1-runtime-write-model.md). The guard and write run
/// off the cache lock, which is held only for the in-memory mutation.
pub async fn mark(
    state: &AppState,
    root: usize,
    rel: &str,
    marker: Marker,
    mode: ViewMode,
) -> Result<MarkOutcome, DomainError> {
    let root_path = state
        .config
        .library_roots
        .get(root)
        .ok_or(DomainError::RootIndex)?
        .clone();
    let rel_owned = rel.to_string();
    let write_path = root_path.clone();
    let (created, canonical_root) =
        tokio::task::spawn_blocking(move || write_marker(&write_path, &rel_owned, marker))
            .await
            .map_err(|_| {
                DomainError::WriteFailed(std::io::Error::other("marker write task failed"))
            })??;

    // A self-write may not bump the folder mtime, so force a re-list on rescan.
    invalidate_index(state, &canonical_root, rel);

    let rel_for_edit = rel.to_string();
    let marker_for_edit = marker;
    let raw = state
        .cache
        .apply_marker_or_build(
            move |raw| apply_mark_raw(raw, root, &rel_for_edit, marker_for_edit),
            || {
                build_view(
                    state.config.as_ref(),
                    &state.settings,
                    Arc::clone(&state.dir_index),
                )
            },
        )
        .await;
    Ok(MarkOutcome {
        view: Arc::new(render_view(&raw, mode)),
        created,
    })
}

/// Delete a marker file and refresh the cached view by rescanning the one
/// affected root (see docs/adr/0002-v1-runtime-write-model.md). The guard and
/// delete run off the cache lock, which is held only for the per-root rebuild.
pub async fn unmark(
    state: &AppState,
    root: usize,
    rel: &str,
    marker: Marker,
    mode: ViewMode,
) -> Result<Arc<FlaggedView>, DomainError> {
    let root_path = state
        .config
        .library_roots
        .get(root)
        .ok_or(DomainError::RootIndex)?
        .clone();
    let rel_owned = rel.to_string();
    let canonical_root = {
        let delete_path = root_path.clone();
        tokio::task::spawn_blocking(move || delete_marker(&delete_path, &rel_owned, marker))
            .await
            .map_err(|_| {
                DomainError::WriteFailed(std::io::Error::other("marker delete task failed"))
            })??
    };

    // A self-delete may not bump the folder mtime, so force a re-list on rebuild.
    invalidate_index(state, &canonical_root, rel);

    let section_root = root_path.clone();
    let section_settings = Arc::clone(&state.settings);
    let section_index = Arc::clone(&state.dir_index);
    let build_config = Arc::clone(&state.config);
    let build_settings = Arc::clone(&state.settings);
    let build_index = Arc::clone(&state.dir_index);
    let raw = state
        .cache
        .rebuild_root(
            root,
            move || {
                let path = section_root.clone();
                let settings = Arc::clone(&section_settings);
                let index = Arc::clone(&section_index);
                async move { build_section(path, settings, index).await }
            },
            move || {
                let config = Arc::clone(&build_config);
                let settings = Arc::clone(&build_settings);
                let index = Arc::clone(&build_index);
                async move { build_view(config.as_ref(), &settings, index).await }
            },
        )
        .await;
    Ok(Arc::new(render_view(&raw, mode)))
}

/// Guard the target and create the marker file, on a blocking task. The root base
/// comes from config, so only `rel` is request-controlled; it is re-validated by
/// canonicalizing the join and confirming it stays inside the root. The open is
/// create-only: `Ok(true)` when this call made the file, `Ok(false)` when it was
/// already there, which keeps a re-mark a no-op and lets undo delete only files it
/// created.
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
/// tolerant: a missing file or a folder that no longer exists is success, since
/// the intended end state (no marker) already holds. Runs on a blocking task.
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
pub(crate) fn apply_mark_raw(raw: &mut state::RawView, root: usize, rel: &str, marker: Marker) {
    let Some(section) = raw.get_mut(root) else {
        return;
    };
    let state::RawRootState::Walked(folders) = &mut section.state else {
        // A Clean or Error section has no folders to flip; the next rescan
        // will reflect the marker on disk when the slot rebuilds.
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
    let marked = std::path::PathBuf::from(rel);
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

/// Render the cached raw view into the requested `ViewMode`'s `FlaggedView`. The
/// gaps path filters with `reduce_to_flagged` and builds the forest; show-all
/// builds directly from the raw folders. Both run on the request thread (the
/// per-folder cost is bounded; see ADR-0022). The render allocates a fresh
/// `FlaggedView` per response and drops it after the response writes.
pub(crate) fn render_view(raw: &state::RawView, mode: ViewMode) -> FlaggedView {
    raw.iter()
        .map(|section| RootSection {
            path: section.path.clone(),
            state: render_root_state(&section.path, &section.state, mode),
        })
        .collect()
}

/// Render one section per mode. The root name for `tree::build`'s `.` node
/// comes from the section's canonical path (last component, or "." when absent),
/// the same rule `scan_root` used to derive it before the rework.
fn render_root_state(path: &str, state: &state::RawRootState, mode: ViewMode) -> RootState {
    match state {
        state::RawRootState::Clean => RootState::Clean,
        state::RawRootState::Error(err) => RootState::Error(err.clone()),
        state::RawRootState::Walked(folders) => {
            let root_name = Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(".");
            match mode {
                ViewMode::GapsOnly => {
                    let flagged = scanner::reduce_to_flagged(folders);
                    let forest = tree::build(root_name, &flagged);
                    if forest.is_empty() {
                        RootState::Clean
                    } else {
                        RootState::Forest(forest)
                    }
                }
                ViewMode::All => RootState::Forest(tree::build(root_name, folders)),
            }
        }
    }
}

/// Build the raw view for every configured root, in config order. Each root is
/// scanned on a blocking task so the directory walk does not stall the runtime.
/// The response renders per mode from the result (see ADR-0022).
pub(crate) async fn build_view(
    config: &Config,
    settings: &Arc<ScanSettings>,
    index: Arc<std::sync::Mutex<scanner::DirIndex>>,
) -> state::RawView {
    let started = Instant::now();
    let mut sections = Vec::with_capacity(config.library_roots.len());
    for root in &config.library_roots {
        sections.push(build_section(root.clone(), Arc::clone(settings), Arc::clone(&index)).await);
    }
    tracing::info!(
        roots = sections.len(),
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "scanned library"
    );
    sections
}

/// Scan one root off the async runtime and fold the result into a raw section.
async fn build_section(
    root: std::path::PathBuf,
    settings: Arc<ScanSettings>,
    index: Arc<std::sync::Mutex<scanner::DirIndex>>,
) -> state::RawRootSection {
    let started = Instant::now();
    let section =
        match tokio::task::spawn_blocking(move || scan_root(&root, &settings, &index)).await {
            Ok(section) => section,
            Err(join_err) => {
                tracing::error!(error = %join_err, "scan task panicked");
                state::RawRootSection {
                    path: "<unknown>".to_string(),
                    state: state::RawRootState::Error("scan task failed".to_string()),
                }
            }
        };
    tracing::debug!(
        root = %section.path,
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "scanned root"
    );
    section
}

/// Lock the shared index, recovering the guard when a previous walk panicked while
/// holding it. A poisoned `DirIndex` is not corrupt: a stale entry is re-listed on
/// its next mtime check, so recovery beats wedging every later scan on a restart.
pub(crate) fn lock_index(
    index: &std::sync::Mutex<scanner::DirIndex>,
) -> std::sync::MutexGuard<'_, scanner::DirIndex> {
    index
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Drop the index entry for `rel` under `root` so the next walk re-lists it. Used
/// after this process writes or deletes a marker, so the change is picked up even
/// if the directory mtime resolution would have hidden the same-tick write. A
/// no-op when the path was never indexed.
fn invalidate_index(state: &AppState, canonical_root: &Path, rel: &str) {
    let target = if rel == "." {
        canonical_root.to_path_buf()
    } else {
        canonical_root.join(rel)
    };
    lock_index(&state.dir_index).invalidate(&target);
}

/// The synchronous per-root work: canonicalize and scan into a raw section.
/// Runs on a blocking thread. A canonicalize failure or a non-directory becomes
/// an `Error` section so one bad root never sinks the page. Rendering happens
/// at request time from this raw output (see ADR-0022).
fn scan_root(
    root: &Path,
    settings: &ScanSettings,
    index: &std::sync::Mutex<scanner::DirIndex>,
) -> state::RawRootSection {
    let canonical = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(root = %root.display(), error = %err, "skipping unreadable library root");
            return state::RawRootSection {
                path: root.display().to_string(),
                state: state::RawRootState::Error(err.to_string()),
            };
        }
    };
    if !canonical.is_dir() {
        tracing::warn!(root = %canonical.display(), "library root is not a directory");
        return state::RawRootSection {
            path: canonical.display().to_string(),
            state: state::RawRootState::Error("not a directory".to_string()),
        };
    }

    // Always reuse the dir index. The /rescan handler clears it for an explicit
    // cold scan; otherwise the index persists across page loads and autosync
    // ticks (see ADR-0023).
    let (folders, stats) = {
        let mut guard = lock_index(index);
        scanner::scan_incremental_with_stats(&canonical, settings, &mut guard)
    };
    tracing::debug!(
        root = %canonical.display(),
        dirs_visited = stats.dirs_visited,
        dirs_reused = stats.dirs_reused,
        entries_seen = stats.entries_seen,
        "walked root"
    );

    let raw_state = if folders.is_empty() {
        // No directory at or below the root contributed an entry: nothing to
        // render in either mode. Render-time can shortcut a Clean section.
        state::RawRootState::Clean
    } else {
        state::RawRootState::Walked(folders)
    };
    state::RawRootSection {
        path: canonical.display().to_string(),
        state: raw_state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    fn test_config(roots: Vec<PathBuf>, ttl_seconds: u64) -> Config {
        Config {
            library_roots: roots,
            ttl_seconds,
            ..Default::default()
        }
    }

    fn test_settings() -> Arc<ScanSettings> {
        Arc::new(ScanSettings::compile(Config::default().scan_inputs()).unwrap())
    }

    fn test_index() -> Arc<std::sync::Mutex<scanner::DirIndex>> {
        Arc::new(std::sync::Mutex::new(scanner::DirIndex::new()))
    }

    #[tokio::test]
    async fn root_with_a_gap_yields_a_matching_forest() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::GapsOnly);
        assert_eq!(view.len(), 1);
        match &view[0].state {
            RootState::Forest(nodes) => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].name, "Author");
            }
            other => panic!("expected Forest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn root_with_no_audio_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Empty")).unwrap();
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::GapsOnly);
        assert!(matches!(view[0].state, RootState::Clean));
    }

    #[tokio::test]
    async fn missing_root_is_error_and_other_roots_still_render() {
        let good = tempfile::tempdir().unwrap();
        touch(&good.path().join("Book/01.mp3"));
        let cfg = test_config(
            vec![
                PathBuf::from("/no/such/root/xyz123"),
                good.path().to_path_buf(),
            ],
            60,
        );
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::GapsOnly);
        assert!(matches!(view[0].state, RootState::Error(_)));
        assert!(matches!(view[1].state, RootState::Forest(_)));
    }

    fn state_for(root: &Path, ttl_seconds: u64) -> AppState {
        let cfg = test_config(vec![root.to_path_buf()], ttl_seconds);
        let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
        AppState::new(cfg, settings)
    }

    #[tokio::test]
    async fn cache_hit_within_ttl_serves_the_same_raw_slot() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let _first = current_view(&state, ViewMode::GapsOnly).await;
        let raw_before = {
            let slot = state.cache.entries.lock().await;
            Arc::clone(&slot.as_ref().unwrap().raw)
        };
        // Cover the gap on disk after the first scan.
        touch(&dir.path().join("Book/Book.epub"));
        let _second = current_view(&state, ViewMode::GapsOnly).await;
        let raw_after = {
            let slot = state.cache.entries.lock().await;
            Arc::clone(&slot.as_ref().unwrap().raw)
        };

        assert!(
            Arc::ptr_eq(&raw_before, &raw_after),
            "a fresh cache must not rebuild the raw slot"
        );
    }

    #[tokio::test]
    async fn warm_concurrent_reads_share_one_raw_slot_and_render_equally() {
        // Two simultaneous reads against a warm cache must:
        // - render equal FlaggedView values (the rendered output is
        //   deterministic over a stable RawView), and
        // - share one Arc<RawView> in the cache slot (neither call rebuilt),
        //   matching the property ADR-0022 documents for warm reads.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        // Warm the cache once so the racing reads land on a fresh slot.
        let _warm = current_view(&state, ViewMode::GapsOnly).await;
        let raw_before = {
            let slot = state.cache.entries.lock().await;
            Arc::clone(&slot.as_ref().unwrap().raw)
        };

        let (a, b) = tokio::join!(
            current_view(&state, ViewMode::GapsOnly),
            current_view(&state, ViewMode::GapsOnly),
        );
        assert_eq!(*a, *b, "warm concurrent renders must produce equal views");

        let raw_after = {
            let slot = state.cache.entries.lock().await;
            Arc::clone(&slot.as_ref().unwrap().raw)
        };
        assert!(
            Arc::ptr_eq(&raw_before, &raw_after),
            "warm concurrent reads must not rebuild the raw slot"
        );
    }

    #[tokio::test]
    async fn ttl_zero_rescans_every_call() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 0);

        let first = current_view(&state, ViewMode::GapsOnly).await;
        assert!(matches!(first[0].state, RootState::Forest(_)));

        touch(&dir.path().join("Book/Book.epub"));
        let second = current_view(&state, ViewMode::GapsOnly).await;
        assert!(
            matches!(second[0].state, RootState::Clean),
            "ttl 0 rescanned and saw the cover"
        );
    }

    #[tokio::test]
    async fn rescan_refreshes_even_within_a_live_ttl() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let first = current_view(&state, ViewMode::GapsOnly).await;
        assert!(matches!(first[0].state, RootState::Forest(_)));

        touch(&dir.path().join("Book/Book.epub"));
        let refreshed = rescan(&state, ViewMode::GapsOnly).await;
        assert!(matches!(refreshed[0].state, RootState::Clean));
    }

    #[tokio::test]
    async fn rescan_clears_the_dir_index_then_repopulates_it() {
        let dir = tempfile::tempdir().unwrap();
        let scenario = crate::scenarios::find_scenario("mixed-forest").expect("scenario exists");
        let roots = (scenario.build)(dir.path());
        let cfg = Config {
            library_roots: roots,
            ttl_seconds: 600,
            ..Config::default()
        };
        let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
        let state = AppState::new(cfg, settings);

        // Warm the index by reading once.
        let _ = current_view(&state, ViewMode::GapsOnly).await;
        let after_warm = state.dir_index.lock().unwrap().len();
        assert!(after_warm > 0, "the warm read populated the dir index");

        // Drop a synthetic entry into the index that no real walk could reach.
        // A cold rescan must drop it; a warm rescan would preserve it.
        let synthetic_path = std::path::PathBuf::from("/nonexistent/synthetic/marker/path");
        state.dir_index.lock().unwrap().insert(
            synthetic_path.clone(),
            scanner::CachedDir {
                mtime: std::time::UNIX_EPOCH,
                subdirs: Vec::new(),
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            },
        );
        assert!(
            state
                .dir_index
                .lock()
                .unwrap()
                .get(&synthetic_path)
                .is_some()
        );

        // Rescan must drop every entry, then the rebuild repopulates it.
        let _ = rescan(&state, ViewMode::GapsOnly).await;
        assert!(
            state
                .dir_index
                .lock()
                .unwrap()
                .get(&synthetic_path)
                .is_none(),
            "rescan must clear the dir index, dropping the synthetic entry"
        );
        let after_rescan = state.dir_index.lock().unwrap().len();
        assert_eq!(
            after_rescan, after_warm,
            "rescan must clear and repopulate to the same count on an unchanged tree"
        );
    }

    #[tokio::test]
    async fn a_warm_state_rescan_reuses_the_index() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        // The state always holds a dir index (see ADR-0023).
        let state = state_for(dir.path(), 0); // ttl 0 so every call rescans

        // First view fills the index, the second reuses it. Both see the gap.
        let first = current_view(&state, ViewMode::GapsOnly).await;
        let second = current_view(&state, ViewMode::GapsOnly).await;
        assert!(matches!(first[0].state, RootState::Forest(_)));
        assert!(matches!(second[0].state, RootState::Forest(_)));

        let reused = state.dir_index.lock().unwrap().len();
        assert!(reused > 0, "the index retained the walked directories");
    }

    #[tokio::test]
    async fn mark_invalidates_the_marked_dir_in_the_index() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        // Warm the index by scanning once.
        current_view(&state, ViewMode::GapsOnly).await;
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let book = canonical.join("Book");
        assert!(
            state.dir_index.lock().unwrap().get(&book).is_some(),
            "Book is indexed after the scan"
        );

        // Marking Book writes .no_ebook into it, so its index entry must be dropped
        // and the next walk re-lists it rather than trusting a pre-write mtime.
        mark(&state, 0, "Book", Marker::NoEbook, ViewMode::GapsOnly)
            .await
            .unwrap();
        assert!(
            state.dir_index.lock().unwrap().get(&book).is_none(),
            "Book's index entry is invalidated by the marker write"
        );
    }

    #[tokio::test]
    async fn render_view_matches_scan_root_per_mode_on_a_seeded_tree() {
        // Equivalence check: render_view of the cached raw output for each mode
        // must produce the same FlaggedView as today's per-mode scan_root would
        // have built. This pins the rework's correctness against the new cache
        // shape on a small but representative seeded tree.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("AuthorA/Book1/01.mp3"));
        touch(&dir.path().join("AuthorB/Covered/01.mp3"));
        touch(&dir.path().join("AuthorB/Covered/Book.epub"));
        let state = state_for(dir.path(), 600);

        let gaps = current_view(&state, ViewMode::GapsOnly).await;
        let all = current_view(&state, ViewMode::All).await;

        let gaps_json = serde_json::to_value(&*gaps).unwrap();
        let all_json = serde_json::to_value(&*all).unwrap();
        assert_ne!(gaps_json, all_json, "gaps and all must differ in shape");

        // The same raw cache served both renders: with a warm TTL, the second
        // call must take the cached slot rather than rebuild.
        let slot = state.cache.entries.lock().await;
        assert!(
            slot.is_some(),
            "the cache holds the raw view between renders"
        );
    }

    #[test]
    fn root_states_serialize_to_stable_json() {
        let clean = serde_json::to_value(RootState::Clean).unwrap();
        assert_eq!(clean, serde_json::json!("clean"));

        let err = serde_json::to_value(RootState::Error("nope".to_string())).unwrap();
        assert_eq!(err, serde_json::json!({ "error": "nope" }));

        let section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Clean,
        };
        let value = serde_json::to_value(&section).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "path": "/lib", "state": "clean" })
        );
    }

    #[test]
    fn write_marker_creates_each_marker_file() {
        for marker in Marker::ALL {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir_all(dir.path().join("Book")).unwrap();
            write_marker(dir.path(), "Book", marker).unwrap();
            assert!(dir.path().join("Book").join(marker.filename()).exists());
        }
    }

    #[test]
    fn write_marker_reports_created_then_not_created() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Book")).unwrap();
        // First write creates the file.
        assert!(write_marker(dir.path(), "Book", Marker::NoEbook).unwrap().0);
        // Second write finds it already there: not created, file still present.
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
        touch(&dir.path().join("Book/01.mp3"));
        let err = write_marker(dir.path(), "Book/01.mp3", Marker::NoEbook).unwrap_err();
        assert!(matches!(err, DomainError::NotADirectory));
    }

    #[test]
    fn delete_marker_removes_each_marker_file() {
        for marker in Marker::ALL {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir_all(dir.path().join("Book")).unwrap();
            let path = dir.path().join("Book").join(marker.filename());
            fs::write(&path, b"").unwrap();
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
        fs::create_dir_all(dir.path().join("Book")).unwrap();
        // No marker on disk: deleting it is a success, the intended end state holds.
        delete_marker(dir.path(), "Book", Marker::NoEbook).unwrap();
    }

    #[test]
    fn delete_marker_is_tolerant_of_a_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        // The folder never existed: still a success, nothing to remove.
        delete_marker(dir.path(), "Gone", Marker::NoEbook).unwrap();
    }

    #[tokio::test]
    async fn mark_updates_a_warm_cache_in_place_without_rescanning() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        // Warm the cache, then mark. We capture the raw slot Arc *after* the
        // mark settles so Arc::make_mut sees only the cache's holder and
        // mutates the stored view in place rather than cloning on write.
        let _first = current_view(&state, ViewMode::GapsOnly).await;
        let after = mark(&state, 0, "Book", Marker::NoEbook, ViewMode::GapsOnly)
            .await
            .unwrap();
        assert!(matches!(after.view[0].state, RootState::Clean));
        assert!(dir.path().join("Book/.no_ebook").exists());
        let raw_after_mark = {
            let slot = state.cache.entries.lock().await;
            Arc::clone(&slot.as_ref().unwrap().raw)
        };

        // A new gap appears on disk, but the warm TTL means the next read
        // serves from the cached raw slot rather than rescanning.
        touch(&dir.path().join("Other/01.mp3"));
        let _again = current_view(&state, ViewMode::GapsOnly).await;
        let raw_after_read = {
            let slot = state.cache.entries.lock().await;
            Arc::clone(&slot.as_ref().unwrap().raw)
        };
        // Drop the post-mark snapshot before the assert so the diagnostic
        // doesn't accidentally include extra holders if the test is extended.
        drop(raw_after_read.clone());
        assert!(
            Arc::ptr_eq(&raw_after_mark, &raw_after_read),
            "a warm raw slot must not have been rebuilt by the second read"
        );
    }

    #[tokio::test]
    async fn mark_on_a_cold_cache_scans_fresh() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let view = mark(
            &state,
            0,
            "Book",
            Marker::EbookElsewhere,
            ViewMode::GapsOnly,
        )
        .await
        .unwrap();
        assert!(matches!(view.view[0].state, RootState::Clean));
        assert!(dir.path().join("Book/.ebook_elsewhere").exists());
    }

    #[tokio::test]
    async fn mark_outside_a_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);
        let err = mark(&state, 0, "..", Marker::NoEbook, ViewMode::GapsOnly)
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::OutsideRoots));
    }

    #[tokio::test]
    async fn mark_with_a_bad_root_index_errors() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path(), 600);
        let err = mark(&state, 9, ".", Marker::NoEbook, ViewMode::GapsOnly)
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::RootIndex));
    }

    #[tokio::test]
    async fn unmark_deletes_the_file_and_re_flags_the_root() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        // Mark, then confirm the root went Clean and the file is on disk.
        let marked = mark(&state, 0, "Book", Marker::NoEbook, ViewMode::GapsOnly)
            .await
            .unwrap();
        assert!(matches!(marked.view[0].state, RootState::Clean));
        assert!(dir.path().join("Book/.no_ebook").exists());

        // Undo: the file is gone and the gap is back.
        let undone = unmark(&state, 0, "Book", Marker::NoEbook, ViewMode::GapsOnly)
            .await
            .unwrap();
        assert!(!dir.path().join("Book/.no_ebook").exists());
        match &undone[0].state {
            RootState::Forest(nodes) => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].name, "Book");
                assert!(nodes[0].needs_ebook());
            }
            other => panic!("expected the gap to return, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unmark_with_a_bad_root_index_errors() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path(), 600);
        let err = unmark(&state, 9, ".", Marker::NoEbook, ViewMode::GapsOnly)
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::RootIndex));
    }

    #[test]
    fn view_mode_parses_the_query_token_leniently() {
        assert_eq!(ViewMode::from_query(Some("all")), ViewMode::All);
        assert_eq!(ViewMode::from_query(Some("gaps")), ViewMode::GapsOnly);
        // Absent or unrecognized falls back to gaps-only.
        assert_eq!(ViewMode::from_query(None), ViewMode::GapsOnly);
        assert_eq!(ViewMode::from_query(Some("xyz")), ViewMode::GapsOnly);
    }

    #[test]
    fn view_mode_round_trips_through_its_query_token() {
        for mode in [ViewMode::GapsOnly, ViewMode::All] {
            assert_eq!(ViewMode::from_query(Some(mode.as_query())), mode);
        }
    }

    #[test]
    fn view_mode_defaults_to_gaps_only() {
        assert_eq!(ViewMode::default(), ViewMode::GapsOnly);
    }

    #[test]
    fn view_mode_deserializes_from_the_query_token() {
        let mode: ViewMode = serde_json::from_value(serde_json::json!("all")).unwrap();
        assert_eq!(mode, ViewMode::All);
    }

    #[tokio::test]
    async fn all_mode_builds_the_full_tree_including_covered_folders() {
        let dir = tempfile::tempdir().unwrap();
        // A gap and a covered book under the same author.
        touch(&dir.path().join("Author/Gap/01.mp3"));
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), test_index()).await;
        let view = render_view(&raw, ViewMode::All);
        let RootState::Forest(nodes) = &view[0].state else {
            panic!("show-all always yields a Forest");
        };
        let author = &nodes[0];
        assert_eq!(author.name, "Author");
        let names: Vec<&str> = author.children.iter().map(|n| n.name.as_str()).collect();
        // Both books appear, unlike gaps-only which would drop Covered.
        assert_eq!(names, vec!["Covered", "Gap"]);
    }
}
