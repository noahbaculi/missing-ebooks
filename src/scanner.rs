//! The scanner: a pure, synchronous directory walk that returns the flagged
//! folders for one library root.
//!
//! A folder is *flagged* when it directly holds audio but is not *covered*.
//! Coverage is an ebook or marker file in the folder or any ancestor up to the
//! root; a covered folder stops the descent, mirroring the reference script's
//! os.walk-with-prune. An excluded directory name or an exclude-glob match also
//! prunes the whole subtree (see docs/adr/0001-exclude-globs-prune-subtrees.md).
//! The walk does not follow symlinks: only real directories are descended, and
//! every non-directory entry is classified by its file name.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use thiserror::Error;

use crate::marker::Marker;

/// The raw, un-normalized lists one scan needs, as named fields so the four
/// string lists cannot be passed in the wrong order. The caller builds this from
/// a `Config`; the scanner stays config-agnostic so its tests stay light.
pub struct ScanInputs<'a> {
    /// Audio extensions; the leading dot is optional and case is ignored.
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
    let name = name.to_string_lossy();
    if Marker::from_filename(name.as_ref()).is_some() {
        return FileKind::Marker;
    }
    if name.starts_with('.') {
        // AppleDouble ._*, hidden sidecars (.beets), .gitkeep: never audio/ebook.
        return FileKind::Other;
    }
    match Path::new(name.as_ref()).extension().and_then(OsStr::to_str) {
        Some(ext) => {
            let ext = ext.to_lowercase();
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

/// One flagged folder from the gaps-only walk: its path relative to the root and the
/// audio filenames it directly holds, natural-sorted. `tree::build` consumes it. The
/// empty relative path is the loose-root case (see ADR-0005).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlaggedFolder {
    /// The folder's path relative to the walked root.
    pub rel_path: PathBuf,
    /// Audio filenames that physically sit in this folder, natural-sorted.
    pub audio_files: Vec<String>,
}

/// Counts from one walk: the directory and entry totals that drive wall time on a
/// network mount, where each is roughly a round trip. The scanner records them so a
/// benchmark can divide its timings without re-walking; production ignores them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WalkStats {
    /// Directories whose entries were read (one successful `read_dir` each).
    pub dirs_visited: usize,
    /// Directory entries iterated across every visited directory, i.e. the number of
    /// `file_type()` calls. Includes files that are neither audio nor ebook.
    pub entries_seen: usize,
    /// Directories served from the index without a listing (incremental rescans).
    /// Zero for a non-incremental walk. `dirs_visited - dirs_reused` is the number
    /// of directories actually read.
    pub dirs_reused: usize,
}

/// One directory's cached facts: its mtime and everything a walk would otherwise
/// re-read. `subdirs` are the non-excluded children; `audio_files` and `cover_files`
/// are already natural-sorted, the same order a fresh listing produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDir {
    /// The directory's modification time when it was last listed.
    pub mtime: std::time::SystemTime,
    /// Non-excluded child directories, to descend without re-listing.
    pub subdirs: Vec<PathBuf>,
    /// Audio filenames physically in this folder, natural-sorted.
    pub audio_files: Vec<String>,
    /// Ebook then marker filenames physically in this folder, natural-sorted.
    pub cover_files: Vec<String>,
}

/// A per-directory cache shared across walks and across both view modes, keyed by
/// the directory's path. A rescan reuses an entry whose mtime still matches and
/// re-lists the rest. In-memory only: it is rebuilt on restart by the startup warm.
#[derive(Debug, Default)]
pub struct DirIndex {
    entries: std::collections::HashMap<PathBuf, CachedDir>,
}

impl DirIndex {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached entry for `dir`, if any.
    #[must_use]
    pub fn get(&self, dir: &Path) -> Option<&CachedDir> {
        self.entries.get(dir)
    }

    /// Insert or replace the entry for `dir`.
    pub fn insert(&mut self, dir: PathBuf, cached: CachedDir) {
        self.entries.insert(dir, cached);
    }

    /// Drop the entry for `dir`, so the next walk re-lists it. Returns whether one
    /// was present.
    pub fn invalidate(&mut self, dir: &Path) -> bool {
        self.entries.remove(dir).is_some()
    }

    /// Number of cached directories.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Walk `root` and return flagged folders, each with the audio filenames it holds,
/// as paths relative to `root`. A flagged folder directly holds audio and is not
/// covered; this filters the full walk, since coverage pruning saves nothing on a
/// flat, wide library (see benchmarks/README.md). Order is unspecified; the tree
/// builder sorts.
#[must_use]
pub fn scan(root: &Path, settings: &ScanSettings) -> Vec<FlaggedFolder> {
    scan_with_stats(root, settings).0
}

/// Like `scan`, but also returns the walk's counts. Runs the one full walk and
/// reduces it to flagged folders, so the gaps view and the full view read the same
/// directories.
#[must_use]
pub fn scan_with_stats(root: &Path, settings: &ScanSettings) -> (Vec<FlaggedFolder>, WalkStats) {
    let (folders, stats) = scan_all_with_stats(root, settings);
    let flagged = folders
        .into_iter()
        .filter(|f| f.directly_holds_audio && f.missing_ebook)
        .map(|f| FlaggedFolder {
            rel_path: f.rel_path,
            audio_files: f.audio_files,
        })
        .collect();
    (flagged, stats)
}

/// One folder from a full walk, tagged with both facts. `scan_all` produces a
/// `Vec<ScannedFolder>`; `tree::build_all` consumes it. The root walked is the
/// empty relative path (see ADR-0005), as `scan` spells the loose-root case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFolder {
    /// The folder's path relative to the walked root.
    pub rel_path: PathBuf,
    /// This folder directly contains at least one audio file.
    pub directly_holds_audio: bool,
    /// No ebook or marker covers it (none here, none in any ancestor).
    pub missing_ebook: bool,
    /// Ebook and marker filenames that physically sit in this folder and cover it
    /// on its own. Ebooks first, then markers, each natural-sorted. Empty for gaps,
    /// plain containers, and folders covered only through an ancestor.
    pub cover_files: Vec<String>,
    /// Audio filenames that physically sit in this folder, natural-sorted. Empty on
    /// a folder with no direct audio. Collected the same way as `cover_files`.
    pub audio_files: Vec<String>,
}

/// Walk `root` and return every folder with both facts, relative to `root`.
/// Unlike `scan`, coverage does not prune the descent: a covered container is
/// still walked down to its book folders, each tagged covered. Excluded names,
/// exclude globs, dot directories, and symlinks still prune. Order is
/// unspecified; the tree builder sorts.
#[must_use]
pub fn scan_all(root: &Path, settings: &ScanSettings) -> Vec<ScannedFolder> {
    scan_all_with_stats(root, settings).0
}

/// Like `scan_all`, but also returns the walk's counts. Non-incremental: every
/// directory is listed (the escape-hatch and the bench baseline use this).
#[must_use]
pub fn scan_all_with_stats(
    root: &Path,
    settings: &ScanSettings,
) -> (Vec<ScannedFolder>, WalkStats) {
    walk_all(root, settings, None)
}

/// The incremental full walk: stat each directory and reuse the index entry when
/// the mtime is unchanged, listing and re-indexing the rest. The same `index`
/// passed across calls makes each rescan cheaper than the last cold walk.
#[must_use]
pub fn scan_all_incremental_with_stats(
    root: &Path,
    settings: &ScanSettings,
    index: &mut DirIndex,
) -> (Vec<ScannedFolder>, WalkStats) {
    walk_all(root, settings, Some(index))
}

/// The incremental gaps walk: the full incremental walk reduced to flagged folders.
#[must_use]
pub fn scan_incremental_with_stats(
    root: &Path,
    settings: &ScanSettings,
    index: &mut DirIndex,
) -> (Vec<FlaggedFolder>, WalkStats) {
    let (folders, stats) = scan_all_incremental_with_stats(root, settings, index);
    let flagged = folders
        .into_iter()
        .filter(|f| f.directly_holds_audio && f.missing_ebook)
        .map(|f| FlaggedFolder {
            rel_path: f.rel_path,
            audio_files: f.audio_files,
        })
        .collect();
    (flagged, stats)
}

/// The level-synchronous breadth-first walk shared by the incremental and
/// non-incremental full scans. Each level is read in parallel with the index
/// borrowed shared; the index is then updated sequentially before descending, so
/// no concurrent mutation is needed (see ADR-0019 for the walk shape).
fn walk_all(
    root: &Path,
    settings: &ScanSettings,
    mut index: Option<&mut DirIndex>,
) -> (Vec<ScannedFolder>, WalkStats) {
    let mut out = Vec::new();
    let mut stats = WalkStats::default();
    let mut frontier: Vec<(PathBuf, bool)> = vec![(root.to_path_buf(), false)];
    while !frontier.is_empty() {
        // Borrow the index shared only for this block, so the mutable update below
        // does not overlap the parallel read.
        let level: Vec<AllDir> = {
            let index_read = index.as_deref();
            frontier
                .par_iter()
                .map(|(dir, covered_from_above)| {
                    read_dir_all(root, dir, *covered_from_above, settings, index_read)
                })
                .collect()
        };
        let mut next = Vec::new();
        for mut dir in level {
            stats.dirs_visited += dir.stats.dirs_visited;
            stats.entries_seen += dir.stats.entries_seen;
            stats.dirs_reused += dir.stats.dirs_reused;
            if let Some(index) = index.as_deref_mut()
                && let Some(cached) = dir.cache_update.take()
            {
                index.insert(dir.path.clone(), cached);
            }
            if let Some(folder) = dir.folder.take() {
                out.push(folder);
            }
            for child in dir.children.drain(..) {
                next.push((child, dir.child_covered));
            }
        }
        frontier = next;
    }
    (out, stats)
}

/// One directory's contribution to the full walk: its tagged folder, the
/// non-excluded children to descend into, the coverage flag those children
/// inherit, the walk counts, and (when listed under an index) the entry to cache.
struct AllDir {
    folder: Option<ScannedFolder>,
    children: Vec<PathBuf>,
    child_covered: bool,
    stats: WalkStats,
    /// The path this `AllDir` describes, so the driver can key the index.
    path: PathBuf,
    /// Set when this directory was listed under an index: the driver stores it.
    /// `None` when reused, when no index is in use, or when the listing failed.
    cache_update: Option<CachedDir>,
}

/// Read and classify one directory for the full walk. With an index, stat the
/// directory first and reuse the cached entry when its mtime is unchanged,
/// listing only on a miss or a changed mtime. Without an index, always list (the
/// escape-hatch path pays no stat). Read-only on the filesystem.
fn read_dir_all(
    root: &Path,
    dir: &Path,
    covered_from_above: bool,
    settings: &ScanSettings,
    index: Option<&DirIndex>,
) -> AllDir {
    if let Some(index) = index {
        if let Ok(mtime) = std::fs::metadata(dir).and_then(|m| m.modified()) {
            if let Some(cached) = index.get(dir)
                && cached.mtime == mtime
            {
                return reuse_dir_all(root, dir, covered_from_above, cached);
            }
            return list_dir_all(root, dir, covered_from_above, settings, Some(mtime));
        }
        // Stat failed (likely unreadable): fall through to a listing, cache nothing.
        return list_dir_all(root, dir, covered_from_above, settings, None);
    }
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
        cover_files: cached.cover_files.clone(),
        audio_files: cached.audio_files.clone(),
    });
    AllDir {
        folder,
        children: cached.subdirs.clone(),
        child_covered: covered,
        stats: WalkStats {
            dirs_visited: 1,
            entries_seen: 0,
            dirs_reused: 1,
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
                children: Vec::new(),
                child_covered: covered_from_above,
                stats: WalkStats::default(),
                path: dir.to_path_buf(),
                cache_update: None,
            };
        }
    };
    let mut stats = WalkStats {
        dirs_visited: 1,
        entries_seen: 0,
        dirs_reused: 0,
    };

    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut audio_files: Vec<String> = Vec::new();
    let mut ebooks: Vec<String> = Vec::new();
    let mut markers: Vec<String> = Vec::new();

    for entry in entries.flatten() {
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
    // tree (see ADR-0005). Warn once, where the full walk reads the root.
    if dir == root && (!ebooks.is_empty() || !markers.is_empty()) {
        tracing::warn!(
            root = %root.display(),
            "a covering ebook or marker sits directly in the library root; \
             this blanks the entire tree (see ADR-0005)"
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
    // order is stable across filesystems.
    ebooks.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));
    markers.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));
    let mut cover_files = ebooks;
    cover_files.extend(markers);

    audio_files.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));

    let children: Vec<PathBuf> = subdirs
        .into_iter()
        .filter(|sub| !is_excluded(root, sub, settings))
        .collect();

    let folder = dir.strip_prefix(root).ok().map(|rel| ScannedFolder {
        rel_path: rel.to_path_buf(),
        directly_holds_audio: !audio_files.is_empty(),
        missing_ebook: !covered,
        cover_files: cover_files.clone(),
        audio_files: audio_files.clone(),
    });

    let cache_update = store_mtime.map(|mtime| CachedDir {
        mtime,
        subdirs: children.clone(),
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

    // The defaults the scanner is normally run with.
    fn default_settings(exclude_globs: &[&str]) -> ScanSettings {
        let audio: Vec<String> = [".mp3", ".m4a", ".m4b", ".flac"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let ebook: Vec<String> = [".epub", ".pdf", ".mobi", ".azw3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let globs: Vec<String> = exclude_globs.iter().map(|s| s.to_string()).collect();
        ScanSettings::compile(ScanInputs {
            audio_exts: &audio,
            ebook_exts: &ebook,
            excluded_dirs: &[],
            exclude_globs: &globs,
        })
        .unwrap()
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    fn flagged_set(root: &Path, settings: &ScanSettings) -> BTreeSet<String> {
        scan(root, settings)
            .iter()
            .map(|f| f.rel_path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    /// Run the gaps walk forced onto a pool of exactly `threads` workers, returning
    /// the full flagged Vec so a test can compare order and per-folder file lists,
    /// not just the flagged set.
    fn scan_on_pool(root: &Path, settings: &ScanSettings, threads: usize) -> Vec<FlaggedFolder> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| scan(root, settings))
    }

    /// Run the full walk forced onto a pool of exactly `threads` workers, returning
    /// the full Vec so a test can compare order, tags, and per-folder file lists.
    fn scan_all_on_pool(
        root: &Path,
        settings: &ScanSettings,
        threads: usize,
    ) -> Vec<ScannedFolder> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| scan_all(root, settings))
    }

    #[test]
    fn parallel_gaps_walk_matches_across_concurrency_levels() {
        // A nested tree with a covered subtree, a glob-pruned subtree, loose audio
        // in the root, and a multi-file folder, so the comparison exercises Vec
        // order, per-folder audio_files, glob pruning, and the flaggable root
        // (ADR-0005) under concurrency, not just the flagged set.
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
        let one = scan_on_pool(dir.path(), &settings, 1);
        let many = scan_on_pool(dir.path(), &settings, 8);
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
                String::new(), // loose root audio (ADR-0005)
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
        assert_eq!(book1.audio_files, vec!["01.mp3", "02.mp3"]);
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
        let (_flagged, stats) = pool.install(|| scan_with_stats(dir.path(), &settings));
        // root + AuthorA + Book1 + Book2.
        assert_eq!(stats.dirs_visited, 4);
    }

    #[test]
    fn audio_only_folder_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert_eq!(got, BTreeSet::from(["Book".to_string()]));
    }

    #[test]
    fn loose_audio_in_the_root_flags_the_root_itself() {
        // Loose audio directly in the root, no author/book folder: the root is
        // the gap, reported as the empty relative path (see ADR-0005).
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("01 - Loose Book.mp3"));
        let got = flagged_set(dir.path(), &default_settings(&[]));
        assert_eq!(got, BTreeSet::from(["".to_string()]));
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
        let audio: Vec<String> = [".mp3"].iter().map(|s| s.to_string()).collect();
        let ebook: Vec<String> = [".epub"].iter().map(|s| s.to_string()).collect();
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
        // The abridged subtree is pruned; (Unabridged) must survive.
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
        scan_all(root, settings)
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
        let one = scan_all_on_pool(dir.path(), &settings, 1);
        let many = scan_all_on_pool(dir.path(), &settings, 8);
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
        assert_eq!(by_path["AuthorB/Covered"].cover_files, vec!["Book.epub"]);
        // AuthorA/Book1 is a gap that keeps both audio files, natural-sorted.
        let book1 = by_path["AuthorA/Book1"];
        assert_eq!(
            (book1.directly_holds_audio, book1.missing_ebook),
            (true, true)
        );
        assert_eq!(book1.audio_files, vec!["01.mp3", "02.mp3"]);
        // The loose-root case: the root (empty path) directly holds audio.
        assert!(by_path[""].directly_holds_audio);
    }

    #[test]
    fn scan_all_tags_a_gap_a_covered_audiobook_and_containers() {
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
    fn scan_all_carries_ancestor_coverage_down_into_a_covered_container() {
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
    fn scan_all_reports_a_plain_container_with_no_audio_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        // An empty-ish folder with no audio and no ebook.
        std::fs::create_dir_all(dir.path().join("Unsorted")).unwrap();
        touch(&dir.path().join("Unsorted/cover.jpg"));
        let got = scanned(dir.path(), &default_settings(&[]));
        assert_eq!(got["Unsorted"], (false, true)); // plain container
    }

    #[test]
    fn scan_all_still_prunes_excluded_dot_and_symlink() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("@eaDir/01.mp3"));
        touch(&dir.path().join(".@__thumb/01.mp3"));
        touch(&dir.path().join("Book/01.mp3"));
        let audio: Vec<String> = [".mp3"].iter().map(|s| s.to_string()).collect();
        let ebook: Vec<String> = [".epub"].iter().map(|s| s.to_string()).collect();
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

    #[test]
    fn scan_all_reports_loose_root_audio_as_the_empty_path() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("01 - Loose.mp3"));
        let got = scanned(dir.path(), &default_settings(&[]));
        // The root itself: holds audio, uncovered.
        assert_eq!(got[""], (true, true));
    }

    #[test]
    #[cfg(unix)]
    fn scan_all_does_not_follow_symlinked_directories() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        touch(&outside.path().join("01.mp3"));
        std::fs::create_dir_all(dir.path().join("Author")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("Author/Linked")).unwrap();
        let got = scanned(dir.path(), &default_settings(&[]));
        assert!(!got.contains_key("Author/Linked"));
    }

    fn cover_files_of(root: &Path, settings: &ScanSettings) -> BTreeMap<String, Vec<String>> {
        scan_all(root, settings)
            .into_iter()
            .map(|f| {
                let rel = f.rel_path.to_string_lossy().replace('\\', "/");
                (rel, f.cover_files)
            })
            .collect()
    }

    #[test]
    fn scan_all_records_ebook_and_marker_filenames_on_the_holding_folder() {
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
    fn scan_all_leaves_cover_files_empty_for_ancestor_covered_folders() {
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
    fn scan_all_lists_ebooks_before_markers_for_different_named_formats() {
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
        let flagged = scan(dir.path(), &default_settings(&[]));
        let book = flagged
            .iter()
            .find(|f| f.rel_path == Path::new("Book"))
            .unwrap();
        assert_eq!(book.audio_files, vec!["01 - One.mp3", "02 - Two.mp3"]);
    }

    #[test]
    fn scan_all_collects_natural_sorted_audio_filenames() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/02 - Two.mp3"));
        touch(&dir.path().join("Book/10 - Ten.mp3"));
        touch(&dir.path().join("Book/01 - One.mp3"));
        let folders = scan_all(dir.path(), &default_settings(&[]));
        let book = folders
            .iter()
            .find(|f| f.rel_path == Path::new("Book"))
            .unwrap();
        assert_eq!(
            book.audio_files,
            vec!["01 - One.mp3", "02 - Two.mp3", "10 - Ten.mp3"]
        );
        // The root container holds no direct audio here, so its list is empty.
        let root = folders
            .iter()
            .find(|f| f.rel_path == Path::new(""))
            .unwrap();
        assert!(root.audio_files.is_empty());
    }

    #[test]
    fn scan_all_with_stats_counts_dirs_and_entries() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Gap/01.mp3"));
        touch(&dir.path().join("Gap/02.mp3"));
        touch(&dir.path().join("Covered/01.mp3"));
        touch(&dir.path().join("Covered/Book.epub"));
        let (_folders, stats) = scan_all_with_stats(dir.path(), &default_settings(&[]));
        assert_eq!(stats.dirs_visited, 3); // root, Gap, Covered
        assert_eq!(stats.entries_seen, 6); // root sees 2 subdirs; Gap and Covered 2 files each
    }

    #[test]
    fn walk_stats_default_has_no_reused_dirs() {
        let stats = WalkStats::default();
        assert_eq!(stats.dirs_reused, 0);
    }

    /// Run the full walk twice against one index. The first call lists everything
    /// and fills the index; the second reuses it with no filesystem change.
    #[test]
    fn incremental_rescan_reuses_unchanged_dirs_and_matches_the_full_walk() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("AuthorA/Book1/01.mp3"));
        touch(&dir.path().join("AuthorB/Covered/01.mp3"));
        touch(&dir.path().join("AuthorB/Covered/Book.epub"));
        let settings = default_settings(&[]);

        let baseline = scan_all(dir.path(), &settings);

        let mut index = DirIndex::new();
        let (first, first_stats) =
            scan_all_incremental_with_stats(dir.path(), &settings, &mut index);
        assert_eq!(
            first, baseline,
            "the first incremental walk equals the full walk"
        );
        assert_eq!(
            first_stats.dirs_reused, 0,
            "nothing to reuse on the first walk"
        );

        let (second, second_stats) =
            scan_all_incremental_with_stats(dir.path(), &settings, &mut index);
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
    fn incremental_rescan_relists_a_changed_dir_and_flips_the_gap() {
        use filetime::{FileTime, set_file_mtime};
        let dir = tempfile::tempdir().unwrap();
        let book = dir.path().join("Author/Book");
        touch(&book.join("01.mp3"));
        let settings = default_settings(&[]);

        let mut index = DirIndex::new();
        let (first, _) = scan_incremental_with_stats(dir.path(), &settings, &mut index);
        assert!(
            first.iter().any(|f| f.rel_path == Path::new("Author/Book")),
            "Author/Book starts as a gap"
        );

        // Cover the gap, then push the folder mtime forward so the change is seen
        // regardless of the filesystem's mtime resolution.
        touch(&book.join("Book.epub"));
        set_file_mtime(&book, FileTime::from_unix_time(4_000_000_000, 0)).unwrap();

        let (second, stats) = scan_incremental_with_stats(dir.path(), &settings, &mut index);
        assert!(
            !second
                .iter()
                .any(|f| f.rel_path == Path::new("Author/Book")),
            "the gap is gone after the ebook lands"
        );
        assert!(
            stats.dirs_reused < stats.dirs_visited,
            "at least one dir was re-listed"
        );
    }

    #[test]
    fn scan_with_stats_now_walks_the_full_tree_like_scan_all() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Series/Series.epub"));
        touch(&dir.path().join("Series/Book 1/01.mp3"));
        let settings = default_settings(&[]);
        let (flagged, gaps_stats) = scan_with_stats(dir.path(), &settings);
        let (_all, all_stats) = scan_all_with_stats(dir.path(), &settings);
        // The gaps view is now a reduction over the full walk, so both read the
        // same directories: root, Series, Book 1.
        assert_eq!(gaps_stats, all_stats);
        assert_eq!(all_stats.dirs_visited, 3);
        // Book 1 holds audio but is covered by the ancestor Series.epub, and Series
        // holds no direct audio, so nothing is flagged.
        assert!(flagged.is_empty());
    }

    #[test]
    fn walk_stats_skip_excluded_subtrees() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("@eaDir/01.mp3"));
        touch(&dir.path().join("@eaDir/02.mp3"));
        touch(&dir.path().join("Book/01.mp3"));
        let audio: Vec<String> = [".mp3"].iter().map(|s| s.to_string()).collect();
        let ebook: Vec<String> = [".epub"].iter().map(|s| s.to_string()).collect();
        let excluded: Vec<String> = vec!["@eadir".to_string()];
        let settings = ScanSettings::compile(ScanInputs {
            audio_exts: &audio,
            ebook_exts: &ebook,
            excluded_dirs: &excluded,
            exclude_globs: &[],
        })
        .unwrap();
        let (_folders, stats) = scan_all_with_stats(dir.path(), &settings);
        // @eaDir is pruned: root and Book are read, the excluded dir is not.
        assert_eq!(stats.dirs_visited, 2); // root, Book
        assert_eq!(stats.entries_seen, 3); // root sees @eaDir + Book; Book sees 01.mp3
    }
}
