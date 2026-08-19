//! The scanner: a pure, synchronous directory walk that returns the flagged
//! folders for one library root.
//!
//! A folder is *flagged* when it directly holds audio but is not *covered*.
//! Coverage is an ebook or marker file in the folder or any ancestor up to the
//! root. A covered folder stops the descent, mirroring the reference script's
//! os.walk-with-prune. An excluded directory name or an exclude-glob match also
//! prunes the whole subtree (see docs/adr/0001-exclude-globs-prune-subtrees.md).
//! The walk does not follow symlinks: only real directories are descended, and
//! every non-directory entry is classified by its file name.
//!
//! One entry point, [`scan_warm`]: stat each directory and reuse the
//! `&DirIndex` entry when the mtime is unchanged, listing the rest. The
//! index is interior-mutable, so a shared reference is enough. Passing a
//! fresh `DirIndex::new()` skips the reuse and walks every directory from
//! scratch, what `docs/CONTEXT.md` calls a cold scan.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use thiserror::Error;

use crate::marker::Marker;

/// The raw, un-normalized lists one scan needs, as named fields so the four
/// string lists cannot be passed in the wrong order. The caller builds this from
/// a `Config`. The scanner stays config-agnostic, so its tests stay light.
#[derive(Clone, Copy)]
pub struct ScanInputs<'a> {
    /// Audio extensions. The leading dot is optional and case is ignored.
    pub audio_exts: &'a [String],
    /// Ebook extensions that count as coverage.
    pub ebook_exts: &'a [String],
    /// Exact directory names to prune (case-insensitive).
    pub excluded_dirs: &'a [String],
    /// Glob patterns that prune a matching folder and its whole subtree.
    pub exclude_globs: &'a [String],
}

/// A failure preparing a scan. It wraps the glob compiler the way `ConfigError`
/// wraps the TOML parser, so `globset` stays behind the seam and callers handle
/// `ScanSettingsError` rather than a `globset::Error`.
#[derive(Debug, Error)]
pub enum ScanSettingsError {
    /// One exclude-glob pattern is not a valid glob.
    #[error("invalid exclude glob {pattern:?}: {source}")]
    InvalidGlob {
        /// The offending glob pattern.
        pattern: String,
        /// Underlying glob-compile error.
        source: globset::Error,
    },
    /// The validated patterns could not be assembled into a match set.
    #[error("could not compile the exclude-glob set: {source}")]
    GlobSet {
        /// Underlying glob-set build error.
        source: globset::Error,
    },
}

/// Prepared, normalized inputs for one scan.
pub struct ScanSettings {
    audio_exts: HashSet<String>,    // lowercase, no leading dot
    ebook_exts: HashSet<String>,    // lowercase, no leading dot
    excluded_dirs: HashSet<String>, // lowercase exact directory names
    exclude_globs: GlobSet,         // case-insensitive, matched on the rel path
}

impl ScanSettings {
    /// Normalize the extension and exclude-name lists and compile the globs.
    ///
    /// # Errors
    /// Returns a [`ScanSettingsError`] if any exclude-glob pattern fails to
    /// compile or the resulting set fails to assemble.
    pub fn compile(inputs: ScanInputs<'_>) -> Result<Self, ScanSettingsError> {
        let mut builder = GlobSetBuilder::new();
        for pattern in inputs.exclude_globs {
            let glob = GlobBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map_err(|source| ScanSettingsError::InvalidGlob {
                    pattern: pattern.clone(),
                    source,
                })?;
            builder.add(glob);
        }
        Ok(Self {
            audio_exts: normalize_exts(inputs.audio_exts),
            ebook_exts: normalize_exts(inputs.ebook_exts),
            excluded_dirs: inputs
                .excluded_dirs
                .iter()
                .map(|d| d.to_lowercase())
                .collect(),
            exclude_globs: builder
                .build()
                .map_err(|source| ScanSettingsError::GlobSet { source })?,
        })
    }

    /// Deepest level the walk descends, counted from the root at zero.
    /// Subtrees below the cap are skipped into `dirs_skipped_depth_capped`, bounding the
    /// per-level render recursion the tree otherwise feeds unchecked.
    // Fixed ceiling. 64 dwarfs any real audiobook layout.
    // Lift into config if a legitimate library ever trips the warning.
    pub(crate) const MAX_DEPTH: usize = 64;
}

fn normalize_exts(exts: &[String]) -> HashSet<String> {
    exts.iter()
        .map(|e| e.trim_start_matches('.').to_lowercase())
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum FileKind {
    Audio,
    Ebook,  // a real ebook file: counts as coverage and is listed by name
    Marker, // a .no_ebook / .ebook_elsewhere marker: counts as coverage
    Other,
}

fn classify_file(name: &OsStr, settings: &ScanSettings) -> FileKind {
    // Marker and dotfile checks need a &str. A non-UTF-8 name is neither, so
    // it falls through to the extension match below.
    if let Some(s) = name.to_str() {
        if Marker::from_filename(s).is_some() {
            return FileKind::Marker;
        }
        if s.starts_with('.') {
            // AppleDouble ._*, hidden sidecars (.beets), .gitkeep: never audio/ebook.
            return FileKind::Other;
        }
    }
    match Path::new(name).extension().and_then(OsStr::to_str) {
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            if settings.ebook_exts.contains(&ext) {
                FileKind::Ebook
            } else if settings.audio_exts.contains(&ext) {
                FileKind::Audio
            } else {
                FileKind::Other
            }
        }
        None => FileKind::Other,
    }
}

/// Counts from one walk: the directory and entry totals that drive wall time on a
/// network mount, where each is roughly a round trip. The scanner records them so a
/// benchmark can divide its timings without re-walking. Production ignores them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WalkStats {
    /// Directories whose entries were read (one successful `read_dir` each).
    pub dirs_visited: usize,
    /// Directory entries iterated across every visited directory, i.e. the number of
    /// `file_type()` calls. Includes files that are neither audio nor ebook.
    pub entries_seen: usize,
    /// Directories served from the index without a listing (warm rescans).
    /// Zero for a cold walk. `dirs_visited - dirs_reused` is the number
    /// of directories actually read.
    pub dirs_reused: usize,
    /// Directories the walk could not read (permission or I/O error).
    /// Nonzero renders the per-root "couldn't be read" warning.
    pub dirs_skipped: usize,
    /// Subtree roots pruned by the depth cap (`ScanSettings::MAX_DEPTH`).
    /// Nonzero renders the per-root "depth limit" warning, distinct from
    /// `dirs_skipped` since the cause and remediation differ.
    pub dirs_skipped_depth_capped: usize,
}

/// One directory's cached facts: its mtime and everything a walk would otherwise
/// re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDir {
    /// The directory's mtime when last walked. A rescan reuses the entry when
    /// this still matches.
    pub mtime: std::time::SystemTime,
    /// The non-excluded children, paths in the same form the walk emitted them.
    /// Shared as `Arc<[PathBuf]>` so a warm reuse copies a pointer rather than
    /// deep-cloning every child path.
    pub subdirs: std::sync::Arc<[PathBuf]>,
    /// Audio filenames, already natural-sorted (the same order a fresh listing
    /// produces).
    pub audio_files: std::sync::Arc<[String]>,
    /// Cover filenames, already natural-sorted (the same order a fresh listing
    /// produces).
    pub cover_files: std::sync::Arc<[String]>,
}

/// A per-directory cache shared across walks and across both view modes, keyed by
/// the directory's path. A rescan reuses an entry whose mtime still matches and
/// re-lists the rest. In-memory only: rebuilt on restart by the startup warm, which
/// is also what reclaims entries for vanished folders, since none are pruned (ADR-0020).
///
/// The map is held behind an internal `Mutex` so the index is `Sync` and the
/// walk takes a shared `&DirIndex`. Poison recovery is a private detail:
/// every method recovers the guard on poison because a stale entry is
/// re-listed on its next mtime check, so recovery beats wedging every later
/// scan.
#[derive(Debug, Default)]
pub struct DirIndex {
    entries: std::sync::Mutex<std::collections::HashMap<PathBuf, CachedDir>>,
}

impl DirIndex {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lock the map, recovering the guard when a previous walk panicked while
    /// holding it. A poisoned index is not corrupt: a stale entry is re-listed
    /// on its next mtime check, so recovery beats wedging every later scan.
    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<PathBuf, CachedDir>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A clone of the cached entry for `dir`, if any. Returns by value because
    /// the entry lives behind the lock. The clone is cheap once the cover and
    /// audio file lists become `Arc<[String]>` (M23, cluster 4).
    #[must_use]
    pub(crate) fn get_cloned(&self, dir: &Path) -> Option<CachedDir> {
        self.lock().get(dir).cloned()
    }

    /// Insert or replace the entry for `dir`.
    pub(crate) fn insert(&self, dir: PathBuf, cached: CachedDir) {
        self.lock().insert(dir, cached);
    }

    /// Drop the entry for `dir`, so the next walk re-lists it. Returns whether one
    /// was present.
    pub(crate) fn invalidate(&self, dir: &Path) -> bool {
        self.lock().remove(dir).is_some()
    }

    /// Drop every cached entry. The next scan walks every directory from
    /// scratch and repopulates the map as it goes.
    pub(crate) fn clear(&self) {
        self.lock().clear();
    }

    /// Number of cached directories.
    #[cfg(test)]
    #[must_use]
    #[allow(clippy::len_without_is_empty)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }
}

/// Reduce a walk's folders to the flagged gaps: those that directly hold
/// audio and lack coverage. Each kept entry is a clone of the original
/// `ScannedFolder` with every fact intact, so the unified tree builder consumes
/// it directly. Borrowing input lets the per-request render path filter the
/// cached raw view without deep-cloning the whole `Vec`.
#[must_use]
pub fn reduce_to_flagged(folders: &[ScannedFolder]) -> Vec<ScannedFolder> {
    folders
        .iter()
        .filter(|f| f.directly_holds_audio && f.missing_ebook)
        .cloned()
        .collect()
}

/// One folder from a walk, tagged with both facts. `scan_warm` returns
/// a `Vec<ScannedFolder>` (with `WalkStats`) that `tree::build`
/// consumes. The root walked is the empty relative path (see ADR-0007),
/// the loose-root case.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScannedFolder {
    /// The folder's path relative to the walked root.
    pub rel_path: PathBuf,
    /// This folder directly contains at least one audio file.
    pub directly_holds_audio: bool,
    /// No ebook or marker covers it (none here, none in any ancestor).
    pub missing_ebook: bool,
    /// Ebook and marker filenames that physically sit in this folder and cover it
    /// on its own. Ebooks first, then markers, each natural-sorted. Empty for gaps,
    /// plain containers, and folders covered only through an ancestor. Shared
    /// with `CachedDir` via `Arc<[String]>` so a folder build and a warm-cache
    /// reuse are pointer copies, not `Vec` clones.
    pub cover_files: std::sync::Arc<[String]>,
    /// Audio filenames that physically sit in this folder, natural-sorted. Empty on
    /// a folder with no direct audio. Shared with `CachedDir` the same way.
    pub audio_files: std::sync::Arc<[String]>,
}

/// The result of scanning one library root: walked folders or a failure message.
///
/// Single owner of the "one root produced one of two outcomes" split: the cache
/// stores a `Vec<RootScan>` (see `raw_view::RawView`), and the renderer consumes
/// it directly.
#[derive(Debug, Clone, Hash)]
pub enum RootScan {
    /// The walk completed. `folders` may be empty when no entry qualified.
    Walked {
        /// The canonicalized root path the walk ran against.
        canonical_path: PathBuf,
        /// Every folder the walk produced. Empty when no entry qualified.
        folders: Vec<ScannedFolder>,
        /// Directories the walk could not read (permission or I/O error).
        skipped_dirs: usize,
        /// Subtree roots pruned by the depth cap.
        depth_capped_dirs: usize,
    },
    /// The root could not be walked.
    Failed {
        /// Canonical when `is_dir` failed post-canonicalize, configured when
        /// canonicalize itself failed.
        path: PathBuf,
        /// The scanner's failure message, surfaced verbatim in the response.
        message: String,
    },
}

impl RootScan {
    /// Returns the display-form path: canonical for `Walked`, best-known for `Failed`.
    #[must_use]
    pub fn display_path(&self) -> std::path::Display<'_> {
        match self {
            RootScan::Walked { canonical_path, .. } => canonical_path.display(),
            RootScan::Failed { path, .. } => path.display(),
        }
    }

    /// Folders directly holding audio. Zero for `Failed`.
    #[must_use]
    pub fn audiobook_count(&self) -> usize {
        self.folders()
            .iter()
            .filter(|f| f.directly_holds_audio)
            .count()
    }

    /// Walked folders, empty for `Failed`.
    #[must_use]
    pub fn folders(&self) -> &[ScannedFolder] {
        match self {
            RootScan::Walked { folders, .. } => folders,
            RootScan::Failed { .. } => &[],
        }
    }
}

/// Scans one library root: canonicalize, verify it is a directory, then walk.
///
/// Returns `Walked` on success (possibly empty when no folder qualified) and
/// `Failed` when canonicalize or the directory check rejected the path. Runs
/// synchronously. The index is interior-mutable, so a shared reference is
/// enough.
///
/// Emits the same tracing events the previous caller did before this move.
#[must_use]
pub(crate) fn scan_root(root: &Path, settings: &ScanSettings, index: &DirIndex) -> RootScan {
    let canonical = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(
                root = %root.display(),
                error = %err,
                "skipping unreadable library root"
            );
            return RootScan::Failed {
                path: root.to_path_buf(),
                message: err.to_string(),
            };
        }
    };
    if !canonical.is_dir() {
        tracing::warn!(root = %canonical.display(), "library root is not a directory");
        return RootScan::Failed {
            path: canonical,
            message: "not a directory".to_string(),
        };
    }
    let (folders, stats) = scan_warm(&canonical, settings, index);
    tracing::debug!(
        root = %canonical.display(),
        dirs_visited = stats.dirs_visited,
        dirs_reused = stats.dirs_reused,
        entries_seen = stats.entries_seen,
        "walked root"
    );
    RootScan::Walked {
        canonical_path: canonical,
        folders,
        skipped_dirs: stats.dirs_skipped,
        depth_capped_dirs: stats.dirs_skipped_depth_capped,
    }
}

/// A warm scan: stat each directory and reuse the `index` entry when the
/// mtime is unchanged, listing and re-indexing the rest. The same `index`
/// passed across calls makes each rescan cheaper than the last cold walk.
/// Pass a fresh `DirIndex::new()` to perform what `docs/CONTEXT.md` calls a
/// cold scan: a walk with no warm cache to consult.
///
/// The walk is level-synchronous breadth-first: each level is read in
/// parallel with the index borrowed shared, then the index is updated
/// sequentially before descending, so no concurrent mutation is needed
/// (see ADR-0019 for the walk shape).
#[must_use]
pub fn scan_warm(
    root: &Path,
    settings: &ScanSettings,
    index: &DirIndex,
) -> (Vec<ScannedFolder>, WalkStats) {
    let mut out = Vec::new();
    let mut stats = WalkStats::default();
    let mut frontier: Vec<(PathBuf, bool)> = vec![(root.to_path_buf(), false)];
    let mut depth = 0usize;
    while !frontier.is_empty() {
        // Read the level in parallel. The index is interior-mutable, so the
        // shared reference works for both the lookups inside `read_dir_all`
        // and the inserts in the sequential pass below.
        let level: Vec<AllDir> = frontier
            .par_iter()
            .map(|(dir, covered_from_above)| {
                read_dir_all(root, dir, *covered_from_above, settings, index)
            })
            .collect();
        let mut next = Vec::new();
        for mut dir in level {
            stats.dirs_visited += dir.stats.dirs_visited;
            stats.entries_seen += dir.stats.entries_seen;
            stats.dirs_reused += dir.stats.dirs_reused;
            stats.dirs_skipped += dir.stats.dirs_skipped;
            stats.dirs_skipped_depth_capped += dir.stats.dirs_skipped_depth_capped;
            if let Some(cached) = dir.cache_update.take() {
                index.insert(dir.path.clone(), cached);
            }
            if let Some(folder) = dir.folder.take() {
                out.push(folder);
            }
            if depth >= ScanSettings::MAX_DEPTH {
                if !dir.children.is_empty() {
                    stats.dirs_skipped_depth_capped += dir.children.len();
                    tracing::warn!(
                        dir = %dir.path.display(),
                        depth,
                        skipped = dir.children.len(),
                        "walk depth cap reached; skipping deeper subtrees"
                    );
                }
            } else {
                for child in dir.children.iter() {
                    next.push((child.clone(), dir.child_covered));
                }
            }
        }
        frontier = next;
        depth += 1;
    }
    (out, stats)
}

/// One directory's contribution to the full walk: its tagged folder, the
/// non-excluded children to descend into, the coverage flag those children
/// inherit, the walk counts, and (when listed under an index) the entry to cache.
struct AllDir {
    folder: Option<ScannedFolder>,
    children: std::sync::Arc<[PathBuf]>,
    child_covered: bool,
    stats: WalkStats,
    /// The path this `AllDir` describes, so the driver can key the index.
    path: PathBuf,
    /// Set when this directory was listed under an index: the driver stores it.
    /// `None` when reused or when the listing failed.
    cache_update: Option<CachedDir>,
}

/// Read and classify one directory for the full walk: stat it first and
/// reuse the cached entry when its mtime is unchanged, listing only on a
/// miss or a changed mtime. Read-only on the filesystem.
fn read_dir_all(
    root: &Path,
    dir: &Path,
    covered_from_above: bool,
    settings: &ScanSettings,
    index: &DirIndex,
) -> AllDir {
    if let Ok(mtime) = std::fs::metadata(dir).and_then(|m| m.modified()) {
        if let Some(cached) = index.get_cloned(dir)
            && cached.mtime == mtime
        {
            return reuse_dir_all(root, dir, covered_from_above, &cached);
        }
        return list_dir_all(root, dir, covered_from_above, settings, Some(mtime));
    }
    // Stat failed (likely unreadable): list without caching.
    list_dir_all(root, dir, covered_from_above, settings, None)
}

/// Build an `AllDir` from a cached entry without touching the directory's contents.
/// Produces the same `ScannedFolder` a fresh listing would, with one stat (the
/// caller already paid it) counted as a reuse.
fn reuse_dir_all(root: &Path, dir: &Path, covered_from_above: bool, cached: &CachedDir) -> AllDir {
    let covered = covered_from_above || !cached.cover_files.is_empty();
    let folder = dir.strip_prefix(root).ok().map(|rel| ScannedFolder {
        rel_path: rel.to_path_buf(),
        directly_holds_audio: !cached.audio_files.is_empty(),
        missing_ebook: !covered,
        cover_files: std::sync::Arc::clone(&cached.cover_files),
        audio_files: std::sync::Arc::clone(&cached.audio_files),
    });
    AllDir {
        folder,
        children: std::sync::Arc::clone(&cached.subdirs),
        child_covered: covered,
        stats: WalkStats {
            dirs_visited: 1,
            entries_seen: 0,
            dirs_reused: 1,
            dirs_skipped: 0,
            dirs_skipped_depth_capped: 0,
        },
        path: dir.to_path_buf(),
        cache_update: None,
    }
}

/// List one directory and classify it for the full walk. When `store_mtime` is
/// `Some`, also produce a `CachedDir` for the driver to index.
fn list_dir_all(
    root: &Path,
    dir: &Path,
    covered_from_above: bool,
    settings: &ScanSettings,
    store_mtime: Option<std::time::SystemTime>,
) -> AllDir {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(dir = %dir.display(), error = %err, "skipping unreadable directory");
            return AllDir {
                folder: None,
                children: std::sync::Arc::from([]),
                child_covered: covered_from_above,
                stats: WalkStats {
                    dirs_skipped: 1,
                    ..WalkStats::default()
                },
                path: dir.to_path_buf(),
                cache_update: None,
            };
        }
    };
    let mut stats = WalkStats {
        dirs_visited: 1,
        entries_seen: 0,
        dirs_reused: 0,
        dirs_skipped: 0,
        dirs_skipped_depth_capped: 0,
    };

    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut audio_files: Vec<String> = Vec::new();
    let mut ebooks: Vec<String> = Vec::new();
    let mut markers: Vec<String> = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(
                    dir = %dir.display(),
                    error = %err,
                    "skipping unreadable entry"
                );
                stats.dirs_skipped += 1;
                continue;
            }
        };
        stats.entries_seen += 1;
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            subdirs.push(entry.path());
        } else {
            let file_name = entry.file_name();
            match classify_file(&file_name, settings) {
                FileKind::Ebook => ebooks.push(file_name.to_string_lossy().into_owned()),
                FileKind::Marker => markers.push(file_name.to_string_lossy().into_owned()),
                FileKind::Audio => audio_files.push(file_name.to_string_lossy().into_owned()),
                FileKind::Other => {}
            }
        }
    }

    // Coverage is monotonic: once an ancestor covers, everything below is covered.
    let covered = covered_from_above || !ebooks.is_empty() || !markers.is_empty();

    // A covering ebook or marker directly in the library root blanks the whole
    // tree (see ADR-0007). Warn once, where the full walk reads the root.
    if dir == root && (!ebooks.is_empty() || !markers.is_empty()) {
        tracing::warn!(
            root = %root.display(),
            "a covering ebook or marker sits directly in the library root; \
             this blanks the entire tree (see ADR-0007)"
        );
    }

    if let Ok(rel) = dir.strip_prefix(root) {
        tracing::trace!(
            dir = %rel.display(),
            subdirs = subdirs.len(),
            audio = audio_files.len(),
            covered,
            "visited directory"
        );
    }

    // Local cover files: ebooks first, then markers, each natural-sorted so the
    // order is stable across filesystems. Both lists are sealed into Arc<[String]>
    // here so the folder and the cache entry share one allocation.
    ebooks.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));
    markers.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));
    let cover_files: std::sync::Arc<[String]> = {
        let mut v = ebooks;
        v.extend(markers);
        v.into()
    };

    audio_files.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));
    let audio_files: std::sync::Arc<[String]> = audio_files.into();

    let children: std::sync::Arc<[PathBuf]> = subdirs
        .into_iter()
        .filter(|sub| !is_excluded(root, sub, settings))
        .collect();

    let folder = dir.strip_prefix(root).ok().map(|rel| ScannedFolder {
        rel_path: rel.to_path_buf(),
        directly_holds_audio: !audio_files.is_empty(),
        missing_ebook: !covered,
        cover_files: std::sync::Arc::clone(&cover_files),
        audio_files: std::sync::Arc::clone(&audio_files),
    });

    let cache_update = store_mtime.map(|mtime| CachedDir {
        mtime,
        subdirs: std::sync::Arc::clone(&children),
        audio_files,
        cover_files,
    });

    AllDir {
        folder,
        children,
        child_covered: covered,
        stats,
        path: dir.to_path_buf(),
        cache_update,
    }
}

fn is_excluded(root: &Path, dir: &Path, settings: &ScanSettings) -> bool {
    if let Some(name) = dir.file_name().and_then(OsStr::to_str) {
        // Dot-prefixed directories (.git, .@__thumb, .stfolder) are skipped
        // without config, mirroring Audiobookshelf's dotpath rule.
        if name.starts_with('.') {
            return true;
        }
        if settings.excluded_dirs.contains(&name.to_lowercase()) {
            return true;
        }
    }
    if let Ok(rel) = dir.strip_prefix(root)
        && settings.exclude_globs.is_match(rel)
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::Path;

    use crate::scenarios::touch;

    // The defaults the scanner is normally run with.
    fn default_settings(exclude_globs: &[&str]) -> ScanSettings {
        let audio: Vec<String> = [".mp3", ".m4a", ".m4b", ".flac"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let ebook: Vec<String> = [".epub", ".pdf", ".mobi", ".azw3"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let globs: Vec<String> = exclude_globs.iter().map(ToString::to_string).collect();
        ScanSettings::compile(ScanInputs {
            audio_exts: &audio,
            ebook_exts: &ebook,
            excluded_dirs: &[],
            exclude_globs: &globs,
        })
        .unwrap()
    }

    fn flagged_set(root: &Path, settings: &ScanSettings) -> BTreeSet<String> {
        scan_warm(root, settings, &DirIndex::new())
            .0
            .iter()
            .filter(|f| f.directly_holds_audio && f.missing_ebook)
            .map(|f| f.rel_path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    /// Run the walk forced onto a pool of exactly `threads` workers, returning
    /// the full Vec so a test can compare order, tags, and per-folder file lists.
    fn scan_on_pool(root: &Path, settings: &ScanSettings, threads: usize) -> Vec<ScannedFolder> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| scan_warm(root, settings, &DirIndex::new()).0)
    }

    /// Push a directory's mtime far into the future so a rescan sees the
    /// change regardless of the filesystem's mtime resolution.
    fn bump_mtime(dir: &Path) {
        std::fs::File::open(dir)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(4_000_000_000))
            .unwrap();
    }

    #[test]
    fn parallel_gaps_walk_matches_across_concurrency_levels() {
        // A nested tree with a covered subtree, a glob-pruned subtree, loose audio
        // in the root, and a multi-file folder, so the comparison exercises Vec
        // order, per-folder audio_files, glob pruning, and the flaggable root
        // (ADR-0007) under concurrency, not just the flagged set.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("AuthorA/Book1/01.mp3"));
        touch(&dir.path().join("AuthorA/Book1/02.mp3"));
        touch(&dir.path().join("AuthorA/Book2/01.mp3"));
        touch(&dir.path().join("AuthorB/Series/Book3/01.mp3"));
        touch(&dir.path().join("AuthorC/Covered/01.mp3"));
        touch(&dir.path().join("AuthorC/Covered/Book.epub"));
        touch(&dir.path().join("Cycle (Abridged)/Book/01.m4b"));
        touch(&dir.path().join("01 - Loose Book.mp3"));
        let settings = default_settings(&["**/*(abridged)*"]);
        let one = reduce_to_flagged(&scan_on_pool(dir.path(), &settings, 1));
        let many = reduce_to_flagged(&scan_on_pool(dir.path(), &settings, 8));
        // The whole Vec, order and each folder's audio_files included: a BTreeSet
        // would hide a reordering or a dropped file.
        assert_eq!(one, many, "concurrency must not change the flagged Vec");
        let flagged: BTreeSet<String> = one
            .iter()
            .map(|f| f.rel_path.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            flagged,
            BTreeSet::from([
                String::new(), // loose root audio (ADR-0007)
                "AuthorA/Book1".to_string(),
                "AuthorA/Book2".to_string(),
                "AuthorB/Series/Book3".to_string(),
            ]),
            "abridged subtree pruned, AuthorC/Covered suppressed"
        );
        let book1 = one
            .iter()
            .find(|f| f.rel_path == Path::new("AuthorA/Book1"))
            .unwrap();
        assert_eq!(book1.audio_files.as_ref(), ["01.mp3", "02.mp3"]);
    }

    #[test]
    fn parallel_gaps_walk_preserves_walk_stats() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("AuthorA/Book1/01.mp3"));
        touch(&dir.path().join("AuthorA/Book2/01.mp3"));
        let settings = default_settings(&[]);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let (_flagged, stats) = pool.install(|| scan_warm(dir.path(), &settings, &DirIndex::new()));
        // root + AuthorA + Book1 + Book2.
        assert_eq!(stats.dirs_visited, 4);
    }

    #[test]
    fn loose_audio_in_the_root_flags_the_root_itself() {
        // Loose audio directly in the root, no author/book folder: the root is
        // the gap, reported as the empty relative path (see ADR-0007).
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("01 - Loose Book.mp3"));
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert_eq!(got, BTreeSet::from([String::new()]));
    }

    #[test]
    fn ebook_beside_audio_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        touch(&dir.path().join("Book/Book.epub"));
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert!(got.is_empty());
    }

    #[test]
    fn ancestor_ebook_covers_descendants() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Series/Series.epub"));
        touch(&dir.path().join("Series/Book 1/01.mp3"));
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert!(
            got.is_empty(),
            "the series ebook covers the book beneath it"
        );
    }

    #[test]
    fn marker_files_suppress_the_flag() {
        for marker in Marker::ALL {
            let dir = tempfile::tempdir().unwrap();
            touch(&dir.path().join("Book/01.mp3"));
            touch(&dir.path().join("Book").join(marker.filename()));
            let got = flagged_set(dir.path(), &default_settings(&[]));
            assert!(
                got.is_empty(),
                "{} should cover the folder",
                marker.filename()
            );
        }
    }

    #[test]
    fn excluded_dir_name_prunes_the_subtree() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("@eaDir/01.mp3"));
        touch(&dir.path().join("Book/01.mp3"));
        let audio: Vec<String> = [".mp3"].iter().map(ToString::to_string).collect();
        let ebook: Vec<String> = [".epub"].iter().map(ToString::to_string).collect();
        let excluded: Vec<String> = vec!["@eadir".to_string()];
        let settings = ScanSettings::compile(ScanInputs {
            audio_exts: &audio,
            ebook_exts: &ebook,
            excluded_dirs: &excluded,
            exclude_globs: &[],
        })
        .unwrap();
        let got = flagged_set(dir.path(), &settings);
        assert_eq!(got, BTreeSet::from(["Book".to_string()]));
    }

    #[test]
    fn exclude_glob_prunes_the_subtree_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Cycle (Abridged)/Book/01.m4b"));
        touch(&dir.path().join("Cycle (Unabridged)/01.m4b"));
        let got = flagged_set(dir.path(), &default_settings(&["**/*(abridged)*"]));
        // The abridged subtree is pruned. (Unabridged) must survive.
        assert_eq!(got, BTreeSet::from(["Cycle (Unabridged)".to_string()]));
    }

    #[test]
    fn dotfiles_and_appledouble_shadows_are_not_audio_or_ebooks() {
        let dir = tempfile::tempdir().unwrap();
        // A real audio file plus an AppleDouble shadow ebook that must not count.
        touch(&dir.path().join("Book/01.m4b"));
        touch(&dir.path().join("Book/._Book.epub"));
        touch(&dir.path().join("Book/.01.m4b.abcd.beets"));
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert_eq!(got, BTreeSet::from(["Book".to_string()]));
    }

    #[test]
    fn dot_prefixed_directories_are_skipped() {
        // A dot-prefixed directory (.git, .@__thumb, .stfolder) is skipped with
        // no config entry, mirroring Audiobookshelf's dotpath rule. Real audio
        // beside it still flags normally.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join(".@__thumb/01.mp3"));
        touch(&dir.path().join("Book/01.mp3"));
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert_eq!(got, BTreeSet::from(["Book".to_string()]));
    }

    #[test]
    fn recognizes_flac_m4a_pdf_mobi_azw3() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Flac/01.flac")); // flagged: audio, no ebook
        touch(&dir.path().join("M4a/01.m4a")); // flagged
        touch(&dir.path().join("Pdf/01.flac"));
        touch(&dir.path().join("Pdf/x.pdf")); // covered by pdf
        touch(&dir.path().join("Mobi/01.flac"));
        touch(&dir.path().join("Mobi/x.mobi")); // covered by mobi
        touch(&dir.path().join("Azw3/01.flac"));
        touch(&dir.path().join("Azw3/x.azw3")); // covered by azw3
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert_eq!(got, BTreeSet::from(["Flac".to_string(), "M4a".to_string()]));
    }

    #[test]
    #[cfg(unix)]
    fn does_not_follow_symlinked_directories() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        touch(&outside.path().join("01.mp3"));
        fs::create_dir_all(dir.path().join("Author")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("Author/Linked")).unwrap();
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert!(
            got.is_empty(),
            "a symlinked directory must not be descended into"
        );
    }

    fn scanned(root: &Path, settings: &ScanSettings) -> BTreeMap<String, (bool, bool)> {
        scan_warm(root, settings, &DirIndex::new())
            .0
            .into_iter()
            .map(|f| {
                let rel = f.rel_path.to_string_lossy().replace('\\', "/");
                (rel, (f.directly_holds_audio, f.missing_ebook))
            })
            .collect()
    }

    #[test]
    fn parallel_full_walk_matches_across_concurrency_levels() {
        // The covered container is still descended (coverage does not prune the
        // full walk), the abridged subtree is glob-pruned, and the root holds loose
        // audio. Comparing the full Vec checks order, tags, cover_files, and
        // audio_files under concurrency, not just the per-folder tag pair.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("AuthorA/Book1/01.mp3"));
        touch(&dir.path().join("AuthorA/Book1/02.mp3"));
        touch(&dir.path().join("AuthorB/Covered/01.mp3"));
        touch(&dir.path().join("AuthorB/Covered/Book.epub"));
        touch(&dir.path().join("AuthorB/Covered/Disc2/02.mp3"));
        touch(&dir.path().join("Cycle (Abridged)/Book/01.m4b"));
        touch(&dir.path().join("01 - Loose Book.mp3"));
        let settings = default_settings(&["**/*(abridged)*"]);
        let one = scan_on_pool(dir.path(), &settings, 1);
        let many = scan_on_pool(dir.path(), &settings, 8);
        assert_eq!(one, many, "concurrency must not change the full walk");
        let by_path: BTreeMap<String, &ScannedFolder> = one
            .iter()
            .map(|f| (f.rel_path.to_string_lossy().replace('\\', "/"), f))
            .collect();
        // The abridged subtree is pruned: neither its container nor its book.
        assert!(!by_path.contains_key("Cycle (Abridged)"));
        assert!(!by_path.contains_key("Cycle (Abridged)/Book"));
        // Disc2 is covered through its ancestor's ebook: holds audio, not missing.
        let disc2 = by_path["AuthorB/Covered/Disc2"];
        assert_eq!(
            (disc2.directly_holds_audio, disc2.missing_ebook),
            (true, false)
        );
        // The covered container lists its own ebook as a cover file.
        assert_eq!(
            by_path["AuthorB/Covered"].cover_files.as_ref(),
            ["Book.epub"]
        );
        // AuthorA/Book1 is a gap that keeps both audio files, natural-sorted.
        let book1 = by_path["AuthorA/Book1"];
        assert_eq!(
            (book1.directly_holds_audio, book1.missing_ebook),
            (true, true)
        );
        assert_eq!(book1.audio_files.as_ref(), ["01.mp3", "02.mp3"]);
        // The loose-root case: the root (empty path) directly holds audio.
        assert!(by_path[""].directly_holds_audio);
    }

    #[test]
    fn scan_tags_a_gap_a_covered_audiobook_and_containers() {
        let dir = tempfile::tempdir().unwrap();
        // Gap: audio, no cover.
        touch(&dir.path().join("Gap Author/Gap Book/01.mp3"));
        // Covered by its own ebook.
        touch(&dir.path().join("Ebook Author/Ebook Book/01.mp3"));
        touch(&dir.path().join("Ebook Author/Ebook Book/Ebook Book.epub"));
        // Covered by its own marker.
        touch(&dir.path().join("Marker Author/Marker Book/01.mp3"));
        touch(&dir.path().join("Marker Author/Marker Book/.no_ebook"));
        let got = scanned(dir.path(), &default_settings(&[]));

        assert_eq!(got["Gap Author"], (false, true)); // plain container
        assert_eq!(got["Gap Author/Gap Book"], (true, true)); // gap
        assert_eq!(got["Ebook Author"], (false, true)); // plain container
        assert_eq!(got["Ebook Author/Ebook Book"], (true, false)); // covered audiobook
        assert_eq!(got["Marker Author/Marker Book"], (true, false)); // covered audiobook
    }

    #[test]
    fn scan_carries_ancestor_coverage_down_into_a_covered_container() {
        let dir = tempfile::tempdir().unwrap();
        // A series-level epub covers the container and every book under it.
        touch(&dir.path().join("Series/Series.epub"));
        touch(&dir.path().join("Series/Book 1/01.mp3"));
        touch(&dir.path().join("Series/Book 2/01.mp3"));
        let got = scanned(dir.path(), &default_settings(&[]));

        assert_eq!(got["Series"], (false, false)); // covered container
        assert_eq!(got["Series/Book 1"], (true, false)); // covered audiobook
        assert_eq!(got["Series/Book 2"], (true, false)); // covered audiobook
    }

    #[test]
    fn scan_reports_a_plain_container_with_no_audio_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        // An empty-ish folder with no audio and no ebook.
        std::fs::create_dir_all(dir.path().join("Unsorted")).unwrap();
        touch(&dir.path().join("Unsorted/cover.jpg"));
        let got = scanned(dir.path(), &default_settings(&[]));
        assert_eq!(got["Unsorted"], (false, true)); // plain container
    }

    #[test]
    fn scan_still_prunes_excluded_dot_and_symlink() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("@eaDir/01.mp3"));
        touch(&dir.path().join(".@__thumb/01.mp3"));
        touch(&dir.path().join("Book/01.mp3"));
        let audio: Vec<String> = [".mp3"].iter().map(ToString::to_string).collect();
        let ebook: Vec<String> = [".epub"].iter().map(ToString::to_string).collect();
        let excluded: Vec<String> = vec!["@eadir".to_string()];
        let settings = ScanSettings::compile(ScanInputs {
            audio_exts: &audio,
            ebook_exts: &ebook,
            excluded_dirs: &excluded,
            exclude_globs: &[],
        })
        .unwrap();
        let got = scanned(dir.path(), &settings);
        assert!(!got.contains_key("@eaDir"), "excluded name is pruned");
        assert!(!got.contains_key(".@__thumb"), "dot directory is pruned");
        assert_eq!(got["Book"], (true, true));
    }

    fn cover_files_of(root: &Path, settings: &ScanSettings) -> BTreeMap<String, Vec<String>> {
        scan_warm(root, settings, &DirIndex::new())
            .0
            .into_iter()
            .map(|f| {
                let rel = f.rel_path.to_string_lossy().replace('\\', "/");
                (rel, f.cover_files.to_vec())
            })
            .collect()
    }

    #[test]
    fn scan_records_ebook_and_marker_filenames_on_the_holding_folder() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Ebook Book/01.mp3"));
        touch(&dir.path().join("Ebook Book/Ebook Book.epub"));
        touch(&dir.path().join("Marker Book/01.mp3"));
        touch(&dir.path().join("Marker Book/.no_ebook"));
        touch(&dir.path().join("Gap Book/01.mp3"));
        let got = cover_files_of(dir.path(), &default_settings(&[]));
        assert_eq!(got["Ebook Book"], vec!["Ebook Book.epub".to_string()]);
        assert_eq!(got["Marker Book"], vec![".no_ebook".to_string()]);
        assert!(got["Gap Book"].is_empty());
    }

    #[test]
    fn scan_leaves_cover_files_empty_for_ancestor_covered_folders() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Series/Series.epub"));
        touch(&dir.path().join("Series/Book 1/01.mp3"));
        let got = cover_files_of(dir.path(), &default_settings(&[]));
        assert_eq!(got["Series"], vec!["Series.epub".to_string()]);
        assert!(
            got["Series/Book 1"].is_empty(),
            "covered from above, no local cover file"
        );
    }

    #[test]
    fn scan_lists_ebooks_before_markers_for_different_named_formats() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        touch(&dir.path().join("Book/Book.epub"));
        touch(&dir.path().join("Book/Book (2016).pdf"));
        touch(&dir.path().join("Book/.no_ebook"));
        let got = cover_files_of(dir.path(), &default_settings(&[]));
        // Both ebooks are listed verbatim, then the marker last.
        assert_eq!(got["Book"].len(), 3);
        assert!(got["Book"][..2].contains(&"Book.epub".to_string()));
        assert!(got["Book"][..2].contains(&"Book (2016).pdf".to_string()));
        assert_eq!(got["Book"][2], ".no_ebook".to_string());
    }

    #[test]
    fn scan_collects_audio_filenames_for_a_flagged_folder() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/02 - Two.mp3"));
        touch(&dir.path().join("Book/01 - One.mp3"));
        let flagged = scan_warm(dir.path(), &default_settings(&[]), &DirIndex::new()).0;
        let book = flagged
            .iter()
            .find(|f| f.rel_path == Path::new("Book"))
            .unwrap();
        assert_eq!(book.audio_files.as_ref(), ["01 - One.mp3", "02 - Two.mp3"]);
    }

    #[test]
    fn scan_collects_natural_sorted_audio_filenames() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/02 - Two.mp3"));
        touch(&dir.path().join("Book/10 - Ten.mp3"));
        touch(&dir.path().join("Book/01 - One.mp3"));
        let folders = scan_warm(dir.path(), &default_settings(&[]), &DirIndex::new()).0;
        let book = folders
            .iter()
            .find(|f| f.rel_path == Path::new("Book"))
            .unwrap();
        assert_eq!(
            book.audio_files.as_ref(),
            ["01 - One.mp3", "02 - Two.mp3", "10 - Ten.mp3"]
        );
        // The root container holds no direct audio here, so its list is empty.
        let root = folders
            .iter()
            .find(|f| f.rel_path == Path::new(""))
            .unwrap();
        assert!(root.audio_files.is_empty());
    }

    #[test]
    fn scan_counts_dirs_and_entries() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Gap/01.mp3"));
        touch(&dir.path().join("Gap/02.mp3"));
        touch(&dir.path().join("Covered/01.mp3"));
        touch(&dir.path().join("Covered/Book.epub"));
        let (_folders, stats) = scan_warm(dir.path(), &default_settings(&[]), &DirIndex::new());
        assert_eq!(stats.dirs_visited, 3); // root, Gap, Covered
        assert_eq!(stats.entries_seen, 6); // root sees 2 subdirs, Gap and Covered 2 files each
    }

    #[test]
    fn walk_stats_default_has_no_reused_dirs() {
        let stats = WalkStats::default();
        assert_eq!(stats.dirs_reused, 0);
    }

    /// Run the full walk twice against one index. The first call lists everything
    /// and fills the index. The second reuses it with no filesystem change.
    #[test]
    fn warm_scan_reuses_unchanged_dirs_and_matches_the_cold_walk() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("AuthorA/Book1/01.mp3"));
        touch(&dir.path().join("AuthorB/Covered/01.mp3"));
        touch(&dir.path().join("AuthorB/Covered/Book.epub"));
        let settings = default_settings(&[]);

        let baseline = scan_warm(dir.path(), &settings, &DirIndex::new()).0;

        let index = DirIndex::new();
        let (first, first_stats) = scan_warm(dir.path(), &settings, &index);
        assert_eq!(first, baseline, "the first warm walk equals the cold walk");
        assert_eq!(
            first_stats.dirs_reused, 0,
            "nothing to reuse on the first walk"
        );

        let (second, second_stats) = scan_warm(dir.path(), &settings, &index);
        assert_eq!(
            second, baseline,
            "an unchanged rescan still equals the full walk"
        );
        assert_eq!(
            second_stats.dirs_reused, second_stats.dirs_visited,
            "every directory is served from the index"
        );
        assert_eq!(second_stats.entries_seen, 0, "no directory was listed");
    }

    /// A directory whose mtime moved is re-listed and its new contents take effect.
    #[test]
    fn warm_scan_relists_a_changed_dir_and_flips_the_gap() {
        let dir = tempfile::tempdir().unwrap();
        let book = dir.path().join("Author/Book");
        touch(&book.join("01.mp3"));
        let settings = default_settings(&[]);

        let index = DirIndex::new();
        let (first, _) = scan_warm(dir.path(), &settings, &index);
        assert!(
            first.iter().any(|f| f.rel_path == Path::new("Author/Book")),
            "Author/Book starts as a gap"
        );

        // Cover the gap, then push the folder mtime forward so the change is seen
        // regardless of the filesystem's mtime resolution.
        touch(&book.join("Book.epub"));
        bump_mtime(&book);

        let (second, stats) = scan_warm(dir.path(), &settings, &index);
        let book_after = second
            .iter()
            .find(|f| f.rel_path == Path::new("Author/Book"))
            .expect("Author/Book is still in the walk, now covered");
        assert!(
            !(book_after.directly_holds_audio && book_after.missing_ebook),
            "the gap is gone after the ebook lands"
        );
        assert!(
            stats.dirs_reused < stats.dirs_visited,
            "at least one dir was re-listed"
        );
    }

    #[test]
    fn warm_scan_relists_a_dir_whose_mtime_moved_backwards() {
        // The index compares mtime by equality, not newer-than, so a clock
        // step or restored backup still re-lists. Pin the safe direction
        let dir = tempfile::tempdir().unwrap();
        let book = dir.path().join("Book");
        touch(&book.join("01.mp3"));
        let settings = default_settings(&[]);
        let index = DirIndex::new();
        let (first, _) = scan_warm(dir.path(), &settings, &index);
        assert!(first.iter().any(|f| f.missing_ebook), "the gap is indexed");

        touch(&book.join("Book.epub"));
        // Push the mtime backwards, before anything the index has seen
        std::fs::File::open(&book)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH)
            .unwrap();
        let (second, _) = scan_warm(dir.path(), &settings, &index);
        let book_folder = second
            .iter()
            .find(|f| f.rel_path.as_os_str() == "Book")
            .unwrap();
        assert!(
            !book_folder.missing_ebook,
            "a backwards mtime must re-list, not reuse the stale entry"
        );
    }

    /// A subdir added under a parent is picked up on rescan once the parent's mtime
    /// moves: it re-lists, its cached subdirs gains the child, and the new folder is
    /// walked and flagged.
    #[test]
    fn warm_scan_picks_up_a_new_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let author = dir.path().join("Author");
        touch(&author.join("Book 1/01.mp3"));
        let settings = default_settings(&[]);

        let index = DirIndex::new();
        let (first, _) = scan_warm(dir.path(), &settings, &index);
        assert!(
            first
                .iter()
                .any(|f| f.rel_path == Path::new("Author/Book 1")),
            "Book 1 is the only gap on the first walk"
        );
        assert!(
            !first
                .iter()
                .any(|f| f.rel_path == Path::new("Author/Book 2")),
            "Book 2 does not exist yet"
        );

        // Add a sibling, then push the parent mtime forward so the change is seen
        // regardless of the filesystem's mtime resolution.
        touch(&author.join("Book 2/01.mp3"));
        bump_mtime(&author);

        let (second, stats) = scan_warm(dir.path(), &settings, &index);
        assert!(
            second
                .iter()
                .any(|f| f.rel_path == Path::new("Author/Book 2")),
            "the new sibling is walked and flagged after the parent re-lists"
        );
        assert!(
            stats.dirs_reused < stats.dirs_visited,
            "the parent was re-listed, not reused from the stale subdirs"
        );
    }

    /// A subdir removed under a parent drops out on rescan once the parent's mtime
    /// moves: it re-lists, its cached subdirs loses the child, and the gone folder is
    /// no longer reported even though its stale index entry lingers.
    #[test]
    fn warm_scan_drops_a_removed_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let author = dir.path().join("Author");
        touch(&author.join("Book 1/01.mp3"));
        touch(&author.join("Book 2/01.mp3"));
        let settings = default_settings(&[]);

        let index = DirIndex::new();
        let (first, _) = scan_warm(dir.path(), &settings, &index);
        assert!(
            first
                .iter()
                .any(|f| f.rel_path == Path::new("Author/Book 2")),
            "Book 2 starts as a gap"
        );

        // Remove the sibling, then push the parent mtime forward.
        std::fs::remove_dir_all(author.join("Book 2")).unwrap();
        bump_mtime(&author);

        let (second, _) = scan_warm(dir.path(), &settings, &index);
        assert!(
            !second
                .iter()
                .any(|f| f.rel_path == Path::new("Author/Book 2")),
            "the removed folder is gone after the parent re-lists"
        );
        assert!(
            second
                .iter()
                .any(|f| f.rel_path == Path::new("Author/Book 1")),
            "the surviving sibling is still flagged"
        );
    }

    #[test]
    fn walk_stats_skip_excluded_subtrees() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("@eaDir/01.mp3"));
        touch(&dir.path().join("@eaDir/02.mp3"));
        touch(&dir.path().join("Book/01.mp3"));
        let audio: Vec<String> = [".mp3"].iter().map(ToString::to_string).collect();
        let ebook: Vec<String> = [".epub"].iter().map(ToString::to_string).collect();
        let excluded: Vec<String> = vec!["@eadir".to_string()];
        let settings = ScanSettings::compile(ScanInputs {
            audio_exts: &audio,
            ebook_exts: &ebook,
            excluded_dirs: &excluded,
            exclude_globs: &[],
        })
        .unwrap();
        let (_folders, stats) = scan_warm(dir.path(), &settings, &DirIndex::new());
        // @eaDir is pruned: root and Book are read, the excluded dir is not.
        assert_eq!(stats.dirs_visited, 2); // root, Book
        assert_eq!(stats.entries_seen, 3); // root sees @eaDir + Book, Book sees 01.mp3
    }

    #[test]
    fn root_scan_walked_carries_canonical_path_and_folders() {
        let folder = ScannedFolder {
            rel_path: ".".into(),
            directly_holds_audio: true,
            cover_files: std::sync::Arc::from(Vec::<String>::new()),
            audio_files: std::sync::Arc::from(Vec::<String>::new()),
            missing_ebook: true,
        };
        let scan = RootScan::Walked {
            canonical_path: PathBuf::from("/lib/audio"),
            folders: vec![folder],
            skipped_dirs: 0,
            depth_capped_dirs: 0,
        };
        assert_eq!(scan.display_path().to_string(), "/lib/audio");
        assert_eq!(scan.audiobook_count(), 1);
        assert_eq!(scan.folders().len(), 1);
    }

    #[test]
    fn root_scan_failed_carries_best_known_path_and_message() {
        let scan = RootScan::Failed {
            path: PathBuf::from("/lib/missing"),
            message: "no such file".to_string(),
        };
        assert_eq!(scan.display_path().to_string(), "/lib/missing");
        assert_eq!(scan.audiobook_count(), 0);
        assert!(scan.folders().is_empty());
    }

    #[test]
    fn root_scan_walked_empty_is_zero_audiobooks() {
        let scan = RootScan::Walked {
            canonical_path: PathBuf::from("/lib"),
            folders: Vec::new(),
            skipped_dirs: 0,
            depth_capped_dirs: 0,
        };
        assert_eq!(scan.audiobook_count(), 0);
        assert!(scan.folders().is_empty());
    }

    #[test]
    fn a_file_root_fails_the_scan_as_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("root.txt");
        std::fs::write(&file, b"").unwrap();
        let scan = scan_root(&file, &default_settings(&[]), &DirIndex::new());
        let RootScan::Failed { message, .. } = scan else {
            panic!("expected Failed for a file root");
        };
        assert_eq!(message, "not a directory");
    }

    #[test]
    fn audiobook_count_counts_walked_folders_that_directly_hold_audio() {
        let walked = RootScan::Walked {
            canonical_path: PathBuf::from("/lib"),
            folders: vec![
                ScannedFolder {
                    rel_path: PathBuf::from("A/Book1"),
                    directly_holds_audio: true,
                    missing_ebook: true,
                    cover_files: std::sync::Arc::from(Vec::<String>::new()),
                    audio_files: std::sync::Arc::from(vec!["01.mp3".to_string()]),
                },
                ScannedFolder {
                    rel_path: PathBuf::from("A"),
                    directly_holds_audio: false,
                    missing_ebook: false,
                    cover_files: std::sync::Arc::from(Vec::<String>::new()),
                    audio_files: std::sync::Arc::from(Vec::<String>::new()),
                },
                ScannedFolder {
                    rel_path: PathBuf::from("A/Book2"),
                    directly_holds_audio: true,
                    missing_ebook: false,
                    cover_files: std::sync::Arc::from(vec!["Book2.epub".to_string()]),
                    audio_files: std::sync::Arc::from(vec!["01.mp3".to_string()]),
                },
            ],
            skipped_dirs: 0,
            depth_capped_dirs: 0,
        };
        assert_eq!(walked.audiobook_count(), 2);
        let empty_walked = RootScan::Walked {
            canonical_path: PathBuf::from("/lib"),
            folders: Vec::new(),
            skipped_dirs: 0,
            depth_capped_dirs: 0,
        };
        assert_eq!(empty_walked.audiobook_count(), 0);
        let failed = RootScan::Failed {
            path: PathBuf::from("/lib"),
            message: "nope".to_string(),
        };
        assert_eq!(failed.audiobook_count(), 0);
    }

    #[test]
    fn scan_warm_reuses_an_unchanged_tree() {
        // Pins the reuse invariant the scan_bench `scan_warm` group depends
        // on: after the priming walk the index serves every directory without
        // a listing, and the reused walk still produces the right gaps.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Gap/01.mp3"));
        touch(&dir.path().join("Covered/01.mp3"));
        touch(&dir.path().join("Covered/Book.epub"));
        let settings = default_settings(&[]);
        let index = DirIndex::new();
        let (_, first) = scan_warm(dir.path(), &settings, &index);
        assert_eq!(first.dirs_reused, 0);
        let (folders, second) = scan_warm(dir.path(), &settings, &index);
        assert_eq!(second.dirs_visited, 3);
        assert_eq!(second.dirs_reused, 3);
        assert_eq!(second.entries_seen, 0);
        let flagged = reduce_to_flagged(&folders);
        assert_eq!(flagged.len(), 1);
    }

    #[test]
    #[cfg(unix)]
    fn an_unreadable_subdirectory_is_counted_and_siblings_still_scan() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Locked/Book/01.mp3"));
        touch(&dir.path().join("Open/01.mp3"));
        let locked = dir.path().join("Locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root (or CAP_DAC_OVERRIDE) reads through the chmod. Nothing to observe.
        if std::fs::read_dir(&locked).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let settings = default_settings(&[]);
        let (folders, stats) = scan_warm(dir.path(), &settings, &DirIndex::new());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(stats.dirs_skipped, 1, "the unreadable directory is counted");
        assert_eq!(stats.dirs_skipped_depth_capped, 0);
        assert!(
            folders.iter().any(|f| f.rel_path.as_os_str() == "Open"),
            "the readable sibling still scans"
        );
        assert!(
            !folders
                .iter()
                .any(|f| f.rel_path.to_string_lossy().starts_with("Locked/")),
            "nothing below the unreadable directory is listed"
        );
    }

    #[test]
    fn walk_depth_caps_at_max_depth_and_counts_the_skipped_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let mut deep = dir.path().to_path_buf();
        // d0 sits at depth 1, so d64 sits at depth 65, one past the cap
        for i in 0..=ScanSettings::MAX_DEPTH {
            deep.push(format!("d{i}"));
        }
        touch(&deep.join("01.mp3"));
        let settings = default_settings(&[]);
        let (folders, stats) = scan_warm(dir.path(), &settings, &DirIndex::new());

        assert_eq!(stats.dirs_skipped, 0);
        assert_eq!(
            stats.dirs_skipped_depth_capped, 1,
            "the capped subtree root is counted"
        );
        assert!(
            folders
                .iter()
                .all(|f| f.rel_path.components().count() <= ScanSettings::MAX_DEPTH),
            "no folder below the cap is listed"
        );
    }
}
