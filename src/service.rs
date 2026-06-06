//! Web-agnostic service layer: the read view types and the typed operations
//! (current view, marker write) shared by the HTML UI and a future JSON API.
//! This increment builds the read path; the marker write arrives in a later one.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;

use crate::config::Config;
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
    let root_name = canonical.file_name().and_then(|n| n.to_str()).unwrap_or(".");
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
        let mut cfg = Config::default();
        cfg.library_roots = roots;
        cfg.ttl_seconds = ttl_seconds;
        cfg
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
            vec![PathBuf::from("/no/such/root/xyz123"), good.path().to_path_buf()],
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

        assert!(Arc::ptr_eq(&first, &second), "a fresh cache must not rescan");
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
        assert_eq!(value, serde_json::json!({ "path": "/lib", "state": "clean" }));
    }
}
