//! The raw scan output type and the pure rule that mutates it for a marker
//! write. `RawView` is one `RootScan` per configured library root, in config
//! order. `apply_mark_raw` applies a marker write in place, honoring the
//! ADR-0005 root-mark rule (`rel == "."` covers the whole root) and the
//! component-aware ancestor rule (`Author` does not cover `Authority/X`).
//! Consumed by `state::RawViewStore` (which memoizes it and edits it in
//! place on `write_mark`) and by `demo::overlay` (which replays against it
//! as the semantic oracle).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures_util::future::join_all;

use crate::config::Config;
use crate::marker::Marker;
use crate::scanner::{self, DirIndex, RootScan, ScanSettings};

/// The whole raw view: one `RootScan` per configured library root, in config order.
pub(crate) type RawView = Vec<RootScan>;

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
        // A Failed section has no folders to flip. The next rescan will
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
            // Authority/X. This is the correctness pin against a naive str cmp.
            folder.missing_ebook = false;
        }
    }
}

fn add_marker(cover_files: &mut Arc<[String]>, marker: Marker) {
    let name = marker.filename().to_string();
    if !cover_files.iter().any(|existing| existing == &name) {
        // An Arc<[String]> is fixed-length, so rebuild on the rare mark. The
        // hot path is the read side, which clones a pointer. This path runs
        // only on the user clicking a marker button.
        let mut next: Vec<String> = cover_files.to_vec();
        next.push(name);
        *cover_files = next.into();
    }
}

/// Build the raw view for every configured root, in config order. Roots are
/// disjoint subtrees, so each scans against its own persistent `DirIndex` on
/// its own blocking task. `join_all` runs the walks in parallel.
pub(crate) async fn build_view(
    config: &Config,
    settings: &Arc<ScanSettings>,
    indices: &[Arc<DirIndex>],
) -> RawView {
    let started = Instant::now();
    let sections = join_all(
        config
            .library_roots
            .iter()
            .zip(indices)
            .map(|(root, index)| {
                build_section(root.clone(), Arc::clone(settings), Arc::clone(index))
            }),
    )
    .await;
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
    index: Arc<DirIndex>,
) -> RootScan {
    let started = Instant::now();
    let scan =
        match tokio::task::spawn_blocking(move || scanner::scan_root(&root, &settings, &index))
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
