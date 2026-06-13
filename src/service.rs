//! Web-agnostic service layer: the read view types and the typed operations
//! (current view, marker write) shared by the HTML UI and a future JSON API.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::Config;
use crate::marker::Marker;
use crate::scanner::{self, ScanSettings};
use crate::state::AppState;
use crate::tree::{self, Node};

/// Which view a read or write targets: today's gaps-only forest, or the full
/// show-all tree. Selects the scan pipeline, the cache slot, and the rendering.
/// Deserializes from the `view` form field; `from_query` is the lenient path for
/// the URL query, where an absent or unknown value falls back to gaps-only.
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

/// A failure performing a write action. The HTML surface renders it as an inline
/// error; a future JSON API would render it as an error body (see the spec).
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
    state
        .cache
        .get_or_build(mode, || {
            build_view(state.config.as_ref(), &state.settings, mode)
        })
        .await
}

/// Force a fresh scan, store it, and return it, ignoring the TTL.
pub async fn rescan(state: &AppState, mode: ViewMode) -> Arc<FlaggedView> {
    state
        .cache
        .rebuild(mode, || {
            build_view(state.config.as_ref(), &state.settings, mode)
        })
        .await
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
/// rescan (see docs/adr/0002-v1-runtime-write-model.md). The guard and the write
/// run off the cache lock; the lock is held only for the in-memory mutation.
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
    let created = tokio::task::spawn_blocking(move || write_marker(&root_path, &rel_owned, marker))
        .await
        .map_err(|_| {
            DomainError::WriteFailed(std::io::Error::other("marker write task failed"))
        })??;

    let view = state
        .cache
        .edit_both_or_build(
            mode,
            |view| apply_mark(&mut view[root], rel),
            |view| apply_mark_all(&mut view[root], rel, marker),
            || build_view(state.config.as_ref(), &state.settings, mode),
        )
        .await;
    Ok(MarkOutcome { view, created })
}

/// Delete a marker file and refresh the cached view by rescanning the one
/// affected root (see docs/adr/0002-v1-runtime-write-model.md). The guard and the
/// delete run off the cache lock; the lock is held only for the per-root rebuild.
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
    {
        let delete_path = root_path.clone();
        tokio::task::spawn_blocking(move || delete_marker(&delete_path, &rel_owned, marker))
            .await
            .map_err(|_| {
                DomainError::WriteFailed(std::io::Error::other("marker delete task failed"))
            })??;
    }

    let section_root = root_path.clone();
    let section_settings = Arc::clone(&state.settings);
    let build_config = Arc::clone(&state.config);
    let build_settings = Arc::clone(&state.settings);
    Ok(state
        .cache
        .rebuild_root(
            root,
            mode,
            move |m| {
                let path = section_root.clone();
                let settings = Arc::clone(&section_settings);
                async move { build_section(path, settings, m).await }
            },
            move || {
                let config = Arc::clone(&build_config);
                let settings = Arc::clone(&build_settings);
                async move { build_view(config.as_ref(), &settings, mode).await }
            },
        )
        .await)
}

/// Guard the target and create the marker file. Runs on a blocking task: the
/// canonicalize calls and the open touch the filesystem. The root base comes
/// from config, so only `rel` is request-controlled, and it is re-validated by
/// canonicalizing the join and confirming it stays inside the root. The open is
/// create-only: returns `Ok(true)` when this call made the file, `Ok(false)`
/// when it was already there. Create-only keeps a re-mark a no-op and lets undo
/// delete only files its own action created.
fn write_marker(root: &Path, rel: &str, marker: Marker) -> Result<bool, DomainError> {
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
    Ok(created)
}

/// Guard the target and delete the marker file. The guarded mirror of
/// `write_marker`: same canonicalize-and-stay-inside-the-root check. Undo is
/// tolerant: a missing file or a folder that no longer exists is success, since
/// the intended end state (no marker) already holds. Runs on a blocking task.
fn delete_marker(root: &Path, rel: &str, marker: Marker) -> Result<(), DomainError> {
    let started = Instant::now();
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(DomainError::TargetMissing),
    };
    let target = if rel == "." {
        canonical_root.clone()
    } else {
        canonical_root.join(rel)
    };
    let canonical_target = match std::fs::canonicalize(&target) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
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
    Ok(())
}

/// Apply a marker write to one root's section. Marking the root directory covers
/// the whole root (see ADR-0005); otherwise remove the marked folder's subtree
/// from the forest and fall to `Clean` once nothing is left. The forest walk and
/// container pruning live in `tree::remove_subtree`.
pub(crate) fn apply_mark(section: &mut RootSection, rel: &str) {
    if rel == "." {
        section.state = RootState::Clean;
        return;
    }
    let RootState::Forest(forest) = &mut section.state else {
        return;
    };
    tree::remove_subtree(forest, rel);
    if forest.is_empty() {
        section.state = RootState::Clean;
    }
}

/// Apply a marker write to one root's section in the show-all slot. Marking the
/// root directory covers the whole root (every node flips to covered); otherwise
/// the marked folder and its descendants flip to covered and stay visible. The
/// forest walk lives in `tree::cover_subtree` / `tree::cover_all`.
pub(crate) fn apply_mark_all(section: &mut RootSection, rel: &str, marker: Marker) {
    let RootState::Forest(forest) = &mut section.state else {
        return;
    };
    if rel == "." {
        tree::cover_all(forest, marker);
    } else {
        tree::cover_subtree(forest, rel, marker);
    }
}

/// Count gap folders (`Node::needs_ebook`) anywhere in a node slice. A small
/// mirror of the renderer's counter; the service layer stays web-agnostic, so it
/// does not reach into `web::render`.
fn count_gaps(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| usize::from(n.needs_ebook()) + count_gaps(&n.children))
        .sum()
}

/// Gaps in one section. `Clean` and `Error` roots contribute none.
fn section_gaps(section: &RootSection) -> usize {
    match &section.state {
        RootState::Forest(nodes) => count_gaps(nodes),
        RootState::Clean | RootState::Error(_) => 0,
    }
}

/// Build the read view for every configured root, in config order. Each root is
/// scanned on a blocking task so the directory walk does not stall the runtime.
pub(crate) async fn build_view(
    config: &Config,
    settings: &Arc<ScanSettings>,
    mode: ViewMode,
) -> FlaggedView {
    let started = Instant::now();
    let mut sections = Vec::with_capacity(config.library_roots.len());
    for root in &config.library_roots {
        sections.push(build_section(root.clone(), Arc::clone(settings), mode).await);
    }
    let gaps: usize = sections.iter().map(section_gaps).sum();
    tracing::info!(
        roots = sections.len(),
        mode = mode.as_query(),
        gaps,
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "scanned library"
    );
    sections
}

/// Scan one root off the async runtime and fold the result into a section.
async fn build_section(
    root: std::path::PathBuf,
    settings: Arc<ScanSettings>,
    mode: ViewMode,
) -> RootSection {
    let started = Instant::now();
    let section = match tokio::task::spawn_blocking(move || scan_root(&root, &settings, mode)).await
    {
        Ok(section) => section,
        Err(join_err) => {
            tracing::error!(error = %join_err, "scan task panicked");
            RootSection {
                path: "<unknown>".to_string(),
                state: RootState::Error("scan task failed".to_string()),
            }
        }
    };
    tracing::debug!(
        root = %section.path,
        mode = mode.as_query(),
        gaps = section_gaps(&section),
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "scanned root"
    );
    section
}

/// The synchronous per-root work: canonicalize, scan, build the forest. Runs on a
/// blocking thread. A canonicalize failure or a non-directory becomes an `Error`
/// section so one bad root never sinks the page.
fn scan_root(root: &Path, settings: &ScanSettings, mode: ViewMode) -> RootSection {
    let canonical = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(root = %root.display(), error = %err, "skipping unreadable library root");
            return RootSection {
                path: root.display().to_string(),
                state: RootState::Error(err.to_string()),
            };
        }
    };
    if !canonical.is_dir() {
        tracing::warn!(root = %canonical.display(), "library root is not a directory");
        return RootSection {
            path: canonical.display().to_string(),
            state: RootState::Error("not a directory".to_string()),
        };
    }
    let root_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(".");
    let state = match mode {
        ViewMode::GapsOnly => {
            let flagged = scanner::scan(&canonical, settings);
            let forest = tree::build(root_name, &flagged);
            if forest.is_empty() {
                RootState::Clean
            } else {
                RootState::Forest(forest)
            }
        }
        // Show-all always yields a Forest, even an empty one. "Clean" is a
        // gaps-only idea; an all-mode root shows its full structure or, with no
        // folders at all, an empty forest the renderer labels "nothing here".
        ViewMode::All => {
            let folders = scanner::scan_all(&canonical, settings);
            RootState::Forest(tree::build_all(root_name, &folders))
        }
    };
    RootSection {
        path: canonical.display().to_string(),
        state,
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

    #[tokio::test]
    async fn root_with_a_gap_yields_a_matching_forest() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let view = build_view(&cfg, &test_settings(), ViewMode::GapsOnly).await;
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
        let view = build_view(&cfg, &test_settings(), ViewMode::GapsOnly).await;
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
        let view = build_view(&cfg, &test_settings(), ViewMode::GapsOnly).await;
        assert!(matches!(view[0].state, RootState::Error(_)));
        assert!(matches!(view[1].state, RootState::Forest(_)));
    }

    fn state_for(root: &Path, ttl_seconds: u64) -> AppState {
        let cfg = test_config(vec![root.to_path_buf()], ttl_seconds);
        let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
        AppState::new(cfg, settings)
    }

    #[tokio::test]
    async fn cache_hit_within_ttl_returns_the_same_view() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let first = current_view(&state, ViewMode::GapsOnly).await;
        // Cover the gap on disk after the first scan.
        touch(&dir.path().join("Book/Book.epub"));
        let second = current_view(&state, ViewMode::GapsOnly).await;

        assert!(
            Arc::ptr_eq(&first, &second),
            "a fresh cache must not rescan"
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
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(matches!(second[0].state, RootState::Clean));
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

    fn gap_leaf(name: &str, rel: &str) -> Node {
        Node {
            name: name.to_string(),
            rel_path: rel.to_string(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: Vec::new(),
            cover_files: Vec::new(),
            audio_files: Vec::new(),
        }
    }

    #[test]
    fn section_gaps_counts_each_flagged_folder_in_a_forest() {
        let section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![Node {
                name: "Author".to_string(),
                rel_path: "Author".to_string(),
                directly_holds_audio: false,
                missing_ebook: true,
                children: vec![
                    gap_leaf("Book", "Author/Book"),
                    gap_leaf("Two", "Author/Two"),
                ],
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            }]),
        };
        // Two flagged leaves; the bare container holds no direct audio, so it is
        // not itself a gap.
        assert_eq!(section_gaps(&section), 2);
    }

    #[test]
    fn section_gaps_is_zero_for_clean_and_error() {
        let clean = RootSection {
            path: "/a".to_string(),
            state: RootState::Clean,
        };
        let errored = RootSection {
            path: "/b".to_string(),
            state: RootState::Error("nope".to_string()),
        };
        assert_eq!(section_gaps(&clean), 0);
        assert_eq!(section_gaps(&errored), 0);
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
        assert!(write_marker(dir.path(), "Book", Marker::NoEbook).unwrap());
        // Second write finds it already there: not created, file still present.
        assert!(!write_marker(dir.path(), "Book", Marker::NoEbook).unwrap());
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

    #[test]
    fn apply_mark_removes_a_leaf_and_prunes_its_container() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![Node {
                name: "Author".to_string(),
                rel_path: "Author".to_string(),
                directly_holds_audio: false,
                missing_ebook: true,
                children: vec![gap_leaf("Book", "Author/Book")],
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            }]),
        };
        apply_mark(&mut section, "Author/Book");
        assert!(matches!(section.state, RootState::Clean));
    }

    #[test]
    fn apply_mark_keeps_a_flagged_node_when_its_child_goes() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![Node {
                name: "Author".to_string(),
                rel_path: "Author".to_string(),
                directly_holds_audio: true,
                missing_ebook: true,
                children: vec![gap_leaf("Book", "Author/Book")],
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            }]),
        };
        apply_mark(&mut section, "Author/Book");
        match &section.state {
            RootState::Forest(nodes) => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].name, "Author");
                assert!(nodes[0].children.is_empty());
                assert!(nodes[0].needs_ebook());
            }
            other => panic!("expected Forest, got {other:?}"),
        }
    }

    #[test]
    fn apply_mark_on_the_root_sets_clean() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![gap_leaf("Author", "Author")]),
        };
        apply_mark(&mut section, ".");
        assert!(matches!(section.state, RootState::Clean));
    }

    #[tokio::test]
    async fn mark_updates_a_warm_cache_in_place_without_rescanning() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let first = current_view(&state, ViewMode::GapsOnly).await;
        assert!(matches!(first[0].state, RootState::Forest(_)));

        let after = mark(&state, 0, "Book", Marker::NoEbook, ViewMode::GapsOnly)
            .await
            .unwrap();
        assert!(matches!(after.view[0].state, RootState::Clean));
        assert!(dir.path().join("Book/.no_ebook").exists());

        // A new gap appears on disk; the warm TTL means current_view returns the
        // same marked view, proving mark did not trigger a rescan.
        touch(&dir.path().join("Other/01.mp3"));
        let again = current_view(&state, ViewMode::GapsOnly).await;
        assert!(Arc::ptr_eq(&after.view, &again));
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
    fn apply_mark_all_covers_the_addressed_subtree_in_place() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![Node {
                name: "Author".to_string(),
                rel_path: "Author".to_string(),
                directly_holds_audio: false,
                missing_ebook: true,
                children: vec![gap_leaf("Book", "Author/Book")],
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            }]),
        };
        apply_mark_all(&mut section, "Author", Marker::NoEbook);
        match &section.state {
            RootState::Forest(nodes) => {
                assert!(!nodes[0].missing_ebook);
                assert!(!nodes[0].children[0].missing_ebook);
            }
            other => panic!("expected Forest, got {other:?}"),
        }
    }

    #[test]
    fn apply_mark_all_on_the_root_covers_everything() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![gap_leaf("Author", "Author")]),
        };
        apply_mark_all(&mut section, ".", Marker::NoEbook);
        match &section.state {
            RootState::Forest(nodes) => assert!(!nodes[0].missing_ebook),
            other => panic!("expected Forest, got {other:?}"),
        }
    }

    #[test]
    fn apply_mark_all_records_the_written_marker_on_the_row() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![gap_leaf("Book", "Book")]),
        };
        apply_mark_all(&mut section, "Book", Marker::NoEbook);
        let RootState::Forest(nodes) = &section.state else {
            panic!("expected a Forest");
        };
        assert_eq!(nodes[0].cover_files, vec![".no_ebook".to_string()]);
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
        let view = build_view(&cfg, &test_settings(), ViewMode::All).await;
        let RootState::Forest(nodes) = &view[0].state else {
            panic!("show-all always yields a Forest");
        };
        let author = &nodes[0];
        assert_eq!(author.name, "Author");
        let names: Vec<&str> = author.children.iter().map(|n| n.name.as_str()).collect();
        // Both books appear, unlike gaps-only which would drop Covered.
        assert_eq!(names, vec!["Covered", "Gap"]);
    }

    #[tokio::test]
    async fn all_slot_is_cold_until_requested_then_builds_on_first_toggle() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Gap/01.mp3"));
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let state = state_for(dir.path(), 600);

        // Gaps-only read does not populate the all slot.
        let gaps = current_view(&state, ViewMode::GapsOnly).await;
        let RootState::Forest(gaps_nodes) = &gaps[0].state else {
            panic!("expected a Forest");
        };
        assert_eq!(gaps_nodes[0].children.len(), 1, "gaps-only drops Covered");

        // First toggle to all builds the all slot.
        let all = current_view(&state, ViewMode::All).await;
        let RootState::Forest(all_nodes) = &all[0].state else {
            panic!("expected a Forest");
        };
        assert_eq!(all_nodes[0].children.len(), 2, "all shows Covered too");
    }

    #[tokio::test]
    async fn mark_edits_both_warm_slots_and_returns_the_requested_one() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let state = state_for(dir.path(), 600);
        // Warm both slots.
        current_view(&state, ViewMode::GapsOnly).await;
        current_view(&state, ViewMode::All).await;

        // Mark while in all mode: the returned view is the all slot, with the book
        // covered (still visible), not removed.
        let after = mark(&state, 0, "Author/Book", Marker::NoEbook, ViewMode::All)
            .await
            .unwrap();
        let RootState::Forest(nodes) = &after.view[0].state else {
            panic!("expected a Forest in all mode");
        };
        let book = &nodes[0].children[0];
        assert_eq!(book.name, "Book");
        assert!(!book.missing_ebook, "the book is now covered, still shown");

        // The gaps slot was edited too: the book is gone and the root is Clean.
        let gaps = current_view(&state, ViewMode::GapsOnly).await;
        assert!(matches!(gaps[0].state, RootState::Clean));
    }

    #[tokio::test]
    async fn mark_with_the_all_slot_cold_edits_gaps_and_leaves_all_cold() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let state = state_for(dir.path(), 600);
        // Warm only the gaps slot.
        current_view(&state, ViewMode::GapsOnly).await;

        // Mark in gaps mode. The returned view is gaps-only and Clean.
        let after = mark(
            &state,
            0,
            "Author/Book",
            Marker::NoEbook,
            ViewMode::GapsOnly,
        )
        .await
        .unwrap();
        assert!(matches!(after.view[0].state, RootState::Clean));

        // The all slot was cold, so it builds fresh now and already reflects the
        // marker on disk: the book is covered, not a gap.
        let all = current_view(&state, ViewMode::All).await;
        let RootState::Forest(nodes) = &all[0].state else {
            panic!("expected a Forest");
        };
        assert!(!nodes[0].children[0].missing_ebook);
    }
}
