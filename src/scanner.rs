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

/// Walk `root` and return flagged folders as paths relative to `root`. Order is
/// unspecified; the tree builder sorts.
#[must_use]
pub fn scan(root: &Path, settings: &ScanSettings) -> Vec<PathBuf> {
    let mut flagged = Vec::new();
    visit(root, root, settings, &mut flagged);
    flagged
}

fn visit(root: &Path, dir: &Path, settings: &ScanSettings, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // Unreadable directory (for example permission denied): log and skip it.
        Err(err) => {
            tracing::warn!(dir = %dir.display(), error = %err, "skipping unreadable directory");
            return;
        }
    };

    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut has_audio = false;
    let mut covered = false;

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            subdirs.push(entry.path());
        } else {
            match classify_file(&entry.file_name(), settings) {
                FileKind::Ebook | FileKind::Marker => covered = true,
                FileKind::Audio => has_audio = true,
                FileKind::Other => {}
            }
        }
    }

    // A covering ebook or marker stops the descent and suppresses any flag.
    if covered {
        if dir == root {
            tracing::warn!(
                root = %root.display(),
                "a covering ebook or marker sits directly in the library root; \
                 this blanks the entire tree (see ADR-0005)"
            );
        }
        return;
    }
    if has_audio && let Ok(rel) = dir.strip_prefix(root) {
        out.push(rel.to_path_buf());
    }
    // A flag does not stop the descent: a child can be a separate gap.
    for sub in subdirs {
        if is_excluded(root, &sub, settings) {
            continue;
        }
        visit(root, &sub, settings, out);
    }
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
    let mut out = Vec::new();
    visit_all(root, root, false, settings, &mut out);
    out
}

fn visit_all(
    root: &Path,
    dir: &Path,
    covered_from_above: bool,
    settings: &ScanSettings,
    out: &mut Vec<ScannedFolder>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(dir = %dir.display(), error = %err, "skipping unreadable directory");
            return;
        }
    };

    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut audio_files: Vec<String> = Vec::new();
    let mut ebooks: Vec<String> = Vec::new();
    let mut markers: Vec<String> = Vec::new();

    for entry in entries.flatten() {
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

    // Local cover files: ebooks first, then markers, each natural-sorted so the
    // order is stable across filesystems.
    ebooks.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));
    markers.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));
    let mut cover_files = ebooks;
    cover_files.extend(markers);

    audio_files.sort_by(|a, b| lexical_sort::natural_lexical_cmp(a, b));

    if let Ok(rel) = dir.strip_prefix(root) {
        out.push(ScannedFolder {
            rel_path: rel.to_path_buf(),
            directly_holds_audio: !audio_files.is_empty(),
            missing_ebook: !covered,
            cover_files,
            audio_files,
        });
    }
    // Coverage does not stop the descent here; only exclusion does.
    for sub in subdirs {
        if is_excluded(root, &sub, settings) {
            continue;
        }
        visit_all(root, &sub, covered, settings, out);
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
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect()
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
}
