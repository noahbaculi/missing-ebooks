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
    Cover, // an ebook or a marker: anything that makes a folder covered
    Other,
}

fn classify_file(name: &OsStr, settings: &ScanSettings) -> FileKind {
    let name = name.to_string_lossy();
    if Marker::from_filename(name.as_ref()).is_some() {
        return FileKind::Cover;
    }
    if name.starts_with('.') {
        // AppleDouble ._*, hidden sidecars (.beets), .gitkeep: never audio/ebook.
        return FileKind::Other;
    }
    match Path::new(name.as_ref()).extension().and_then(OsStr::to_str) {
        Some(ext) => {
            let ext = ext.to_lowercase();
            if settings.ebook_exts.contains(&ext) {
                FileKind::Cover
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
                FileKind::Cover => covered = true,
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
    use std::collections::BTreeSet;
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
}
