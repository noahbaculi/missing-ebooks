//! The raw view: one `RootScan` per configured library root, in config order
//! (see ADR-0022). This module owns the pure data type and three pure
//! operations on it: an async scan-from-disk into raw form (`build_view`,
//! plus its per-root helper `build_section`) and an in-memory mark edit
//! (`apply_mark_raw`). `RawViewStore` in `state.rs` and the demo handlers
//! both consume this module as peers (see ADR-0029).

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use crate::config::Config;
use crate::marker::Marker;
use crate::scanner::{self, DirIndex, RootScan, ScanSettings};

/// The whole raw view: one `RootScan` per configured library root, in config order.
pub type RawView = Vec<RootScan>;

/// Apply a marker write to the raw view. For a non-root mark with path `P`:
/// every entry whose `rel_path` equals `P` or starts with `P` followed by `/`
/// flips `missing_ebook` to false, and the entry whose `rel_path` equals `P`
/// gains the marker filename in `cover_files`. For a root mark (rel == ".",
/// see ADR-0005): every entry under that root flips `missing_ebook` to false,
/// and the empty-rel-path entry gains the marker filename. Component-aware
/// match: `Author` does not cover `Authority/X`.
pub fn apply_mark_raw(raw: &mut RawView, root: usize, rel: &str, marker: Marker) {
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

/// Build the raw view for every configured root, in config order. Each root
/// is scanned on a blocking task so the directory walk does not stall the
/// runtime. The response renders per mode from the result (see ADR-0022).
pub async fn build_view(
    config: &Config,
    settings: &Arc<ScanSettings>,
    index: Arc<StdMutex<DirIndex>>,
) -> RawView {
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

/// Scans one root off the async runtime and folds the result into a `RootScan`.
pub(crate) async fn build_section(
    root: PathBuf,
    settings: Arc<ScanSettings>,
    index: Arc<StdMutex<DirIndex>>,
) -> RootScan {
    let started = Instant::now();
    let scan = match tokio::task::spawn_blocking(move || {
        let mut guard = lock_index(&index);
        scanner::scan_root(&root, &settings, &mut guard)
    })
    .await
    {
        Ok(scan) => scan,
        Err(join_err) => {
            tracing::error!(error = %join_err, "scan task panicked");
            scanner::RootScan::Failed {
                path: PathBuf::from("<unknown>"),
                message: "scan task failed".to_string(),
            }
        }
    };
    tracing::debug!(
        root = %scan.display_path(),
        elapsed_ms = started.elapsed().as_secs_f64() * 1e3,
        "scanned root"
    );
    scan
}

/// Lock the shared index, recovering the guard when a previous walk panicked
/// while holding it. A poisoned `DirIndex` is not corrupt: a stale entry is
/// re-listed on its next mtime check, so recovery beats wedging every later
/// scan on a restart.
pub(crate) fn lock_index(index: &StdMutex<DirIndex>) -> std::sync::MutexGuard<'_, DirIndex> {
    index
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
