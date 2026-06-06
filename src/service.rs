//! Web-agnostic service layer: the read view types and the typed operations
//! (current view, marker write) shared by the HTML UI and a future JSON API.
//! This increment builds the read path; the marker write arrives in a later one.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use thiserror::Error;

use crate::config::Config;
use crate::marker::Marker;
use crate::scanner::{self, ScanSettings};
use crate::state::{AppState, CacheEntry};
use crate::tree::{self, Node};

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
/// Single-flight: the mutex is held across the scan, so concurrent stale readers
/// block, then re-check and return the view the first scan stored.
pub async fn current_view(state: &AppState) -> Arc<FlaggedView> {
    let mut guard = state.cache.entry.lock().await;
    if let Some(entry) = guard.as_ref()
        && let Some(ttl) = state.cache.ttl
        && entry.stored_at.elapsed() < ttl
    {
        return Arc::clone(&entry.view);
    }
    let view = Arc::new(build_view(state.config.as_ref(), &state.settings).await);
    *guard = Some(CacheEntry {
        stored_at: Instant::now(),
        view: Arc::clone(&view),
    });
    view
}

/// Force a fresh scan, store it, and return it, ignoring the TTL. Shares the
/// cache mutex with `current_view`, so a rescan and a stale read cannot both scan.
pub async fn rescan(state: &AppState) -> Arc<FlaggedView> {
    let mut guard = state.cache.entry.lock().await;
    let view = Arc::new(build_view(state.config.as_ref(), &state.settings).await);
    *guard = Some(CacheEntry {
        stored_at: Instant::now(),
        view: Arc::clone(&view),
    });
    view
}

/// Write a marker into a folder and update the cached view in place, without a
/// rescan (see docs/adr/0002-v1-runtime-write-model.md). The guard and the write
/// run off the cache lock; the lock is held only for the in-memory mutation.
pub async fn mark(
    state: &AppState,
    root: usize,
    rel: &str,
    marker: Marker,
) -> Result<Arc<FlaggedView>, DomainError> {
    let root_path = state
        .config
        .library_roots
        .get(root)
        .ok_or(DomainError::RootIndex)?
        .clone();
    let rel_owned = rel.to_string();
    tokio::task::spawn_blocking(move || write_marker(&root_path, &rel_owned, marker))
        .await
        .map_err(|_| {
            DomainError::WriteFailed(std::io::Error::other("marker write task failed"))
        })??;

    let mut guard = state.cache.entry.lock().await;
    match guard.as_mut() {
        Some(entry) => {
            let mut view = (*entry.view).clone();
            apply_mark(&mut view[root], rel);
            entry.view = Arc::new(view);
            Ok(Arc::clone(&entry.view))
        }
        None => {
            let view = Arc::new(build_view(state.config.as_ref(), &state.settings).await);
            *guard = Some(CacheEntry {
                stored_at: Instant::now(),
                view: Arc::clone(&view),
            });
            Ok(view)
        }
    }
}

/// Guard the target and write the marker file. Runs on a blocking task: the
/// canonicalize calls and the write touch the filesystem. The root base comes
/// from config, so only `rel` is request-controlled, and it is re-validated by
/// canonicalizing the join and confirming it stays inside the root.
fn write_marker(root: &Path, rel: &str, marker: Marker) -> Result<(), DomainError> {
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
    std::fs::write(canonical_target.join(marker.filename()), b"").map_err(DomainError::WriteFailed)
}

/// Remove a marked folder from one root's section, pruning emptied containers. A
/// marker covers the folder and everything beneath it, so removing the node's
/// whole subtree is equivalent to a rescan (see ADR-0002).
fn apply_mark(section: &mut RootSection, rel: &str) {
    if rel == "." {
        // A marker in the root directory covers the whole root (see ADR-0005).
        section.state = RootState::Clean;
        return;
    }
    let RootState::Forest(forest) = &mut section.state else {
        return;
    };
    let components: Vec<&str> = rel.split('/').collect();
    remove_path(forest, &components, "");
    if forest.is_empty() {
        section.state = RootState::Clean;
    }
}

/// Walk the forest by path component, remove the addressed node, and prune any
/// ancestor that is now an empty, non-flagged container. A target that is already
/// gone, because a rescan landed first or a button was double-clicked, is a
/// silent no-op.
fn remove_path(siblings: &mut Vec<Node>, components: &[&str], parent_rel: &str) {
    let Some((head, tail)) = components.split_first() else {
        return;
    };
    let cur_rel = if parent_rel.is_empty() {
        (*head).to_string()
    } else {
        format!("{parent_rel}/{head}")
    };
    let Some(idx) = siblings.iter().position(|n| n.rel_path == cur_rel) else {
        return;
    };
    if tail.is_empty() {
        siblings.remove(idx);
    } else {
        remove_path(&mut siblings[idx].children, tail, &cur_rel);
        if siblings[idx].children.is_empty() && !siblings[idx].flagged {
            siblings.remove(idx);
        }
    }
}

/// Build the read view for every configured root, in config order. Each root is
/// scanned on a blocking task so the directory walk does not stall the runtime.
async fn build_view(config: &Config, settings: &Arc<ScanSettings>) -> FlaggedView {
    let mut sections = Vec::with_capacity(config.library_roots.len());
    for root in &config.library_roots {
        sections.push(build_section(root.clone(), Arc::clone(settings)).await);
    }
    sections
}

/// Scan one root off the async runtime and fold the result into a section.
async fn build_section(root: std::path::PathBuf, settings: Arc<ScanSettings>) -> RootSection {
    match tokio::task::spawn_blocking(move || scan_root(&root, &settings)).await {
        Ok(section) => section,
        Err(join_err) => {
            tracing::error!(error = %join_err, "scan task panicked");
            RootSection {
                path: "<unknown>".to_string(),
                state: RootState::Error("scan task failed".to_string()),
            }
        }
    }
}

/// The synchronous per-root work: canonicalize, scan, build the forest. Runs on a
/// blocking thread. A canonicalize failure or a non-directory becomes an `Error`
/// section so one bad root never sinks the page.
fn scan_root(root: &Path, settings: &ScanSettings) -> RootSection {
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
    let flagged = scanner::scan(&canonical, settings);
    let root_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(".");
    let forest = tree::build(root_name, &flagged);
    let state = if forest.is_empty() {
        RootState::Clean
    } else {
        RootState::Forest(forest)
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
        let defaults = Config::default();
        Arc::new(
            ScanSettings::compile(crate::scanner::ScanInputs {
                audio_exts: &defaults.audio_exts,
                ebook_exts: &defaults.ebook_exts,
                excluded_dirs: &[],
                exclude_globs: &[],
            })
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn root_with_a_gap_yields_a_matching_forest() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let view = build_view(&cfg, &test_settings()).await;
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
        let view = build_view(&cfg, &test_settings()).await;
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
        let view = build_view(&cfg, &test_settings()).await;
        assert!(matches!(view[0].state, RootState::Error(_)));
        assert!(matches!(view[1].state, RootState::Forest(_)));
    }

    fn state_for(root: &Path, ttl_seconds: u64) -> AppState {
        let cfg = test_config(vec![root.to_path_buf()], ttl_seconds);
        let defaults = Config::default();
        let settings = ScanSettings::compile(crate::scanner::ScanInputs {
            audio_exts: &defaults.audio_exts,
            ebook_exts: &defaults.ebook_exts,
            excluded_dirs: &[],
            exclude_globs: &[],
        })
        .unwrap();
        AppState::new(cfg, settings)
    }

    #[tokio::test]
    async fn cache_hit_within_ttl_returns_the_same_view() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let first = current_view(&state).await;
        // Cover the gap on disk after the first scan.
        touch(&dir.path().join("Book/Book.epub"));
        let second = current_view(&state).await;

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

        let first = current_view(&state).await;
        assert!(matches!(first[0].state, RootState::Forest(_)));

        touch(&dir.path().join("Book/Book.epub"));
        let second = current_view(&state).await;
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(matches!(second[0].state, RootState::Clean));
    }

    #[tokio::test]
    async fn rescan_refreshes_even_within_a_live_ttl() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let first = current_view(&state).await;
        assert!(matches!(first[0].state, RootState::Forest(_)));

        touch(&dir.path().join("Book/Book.epub"));
        let refreshed = rescan(&state).await;
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

    fn flagged_leaf(name: &str, rel: &str) -> Node {
        Node {
            name: name.to_string(),
            rel_path: rel.to_string(),
            flagged: true,
            children: Vec::new(),
        }
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
    fn apply_mark_removes_a_leaf_and_prunes_its_container() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![Node {
                name: "Author".to_string(),
                rel_path: "Author".to_string(),
                flagged: false,
                children: vec![flagged_leaf("Book", "Author/Book")],
            }]),
        };
        apply_mark(&mut section, "Author/Book");
        assert!(matches!(section.state, RootState::Clean));
    }

    #[test]
    fn apply_mark_on_a_container_removes_the_whole_subtree() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![Node {
                name: "Author".to_string(),
                rel_path: "Author".to_string(),
                flagged: false,
                children: vec![
                    flagged_leaf("Book 1", "Author/Book 1"),
                    flagged_leaf("Book 2", "Author/Book 2"),
                ],
            }]),
        };
        apply_mark(&mut section, "Author");
        assert!(matches!(section.state, RootState::Clean));
    }

    #[test]
    fn apply_mark_keeps_a_flagged_node_when_its_child_goes() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![Node {
                name: "Author".to_string(),
                rel_path: "Author".to_string(),
                flagged: true,
                children: vec![flagged_leaf("Book", "Author/Book")],
            }]),
        };
        apply_mark(&mut section, "Author/Book");
        match &section.state {
            RootState::Forest(nodes) => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].name, "Author");
                assert!(nodes[0].children.is_empty());
                assert!(nodes[0].flagged);
            }
            other => panic!("expected Forest, got {other:?}"),
        }
    }

    #[test]
    fn apply_mark_on_the_root_sets_clean() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![flagged_leaf("Author", "Author")]),
        };
        apply_mark(&mut section, ".");
        assert!(matches!(section.state, RootState::Clean));
    }

    #[test]
    fn apply_mark_on_an_absent_path_is_a_noop() {
        let mut section = RootSection {
            path: "/lib".to_string(),
            state: RootState::Forest(vec![flagged_leaf("Author", "Author")]),
        };
        apply_mark(&mut section, "Ghost");
        match &section.state {
            RootState::Forest(nodes) => assert_eq!(nodes.len(), 1),
            other => panic!("expected Forest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mark_updates_a_warm_cache_in_place_without_rescanning() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let first = current_view(&state).await;
        assert!(matches!(first[0].state, RootState::Forest(_)));

        let after = mark(&state, 0, "Book", Marker::NoEbook).await.unwrap();
        assert!(matches!(after[0].state, RootState::Clean));
        assert!(dir.path().join("Book/.no_ebook").exists());

        // A new gap appears on disk; the warm TTL means current_view returns the
        // same marked view, proving mark did not trigger a rescan.
        touch(&dir.path().join("Other/01.mp3"));
        let again = current_view(&state).await;
        assert!(Arc::ptr_eq(&after, &again));
    }

    #[tokio::test]
    async fn mark_on_a_cold_cache_scans_fresh() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);

        let view = mark(&state, 0, "Book", Marker::EbookElsewhere)
            .await
            .unwrap();
        assert!(matches!(view[0].state, RootState::Clean));
        assert!(dir.path().join("Book/.ebook_elsewhere").exists());
    }

    #[tokio::test]
    async fn mark_outside_a_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let state = state_for(dir.path(), 600);
        let err = mark(&state, 0, "..", Marker::NoEbook).await.unwrap_err();
        assert!(matches!(err, DomainError::OutsideRoots));
    }

    #[tokio::test]
    async fn mark_with_a_bad_root_index_errors() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path(), 600);
        let err = mark(&state, 9, ".", Marker::NoEbook).await.unwrap_err();
        assert!(matches!(err, DomainError::RootIndex));
    }
}
