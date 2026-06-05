//! Configuration: built-in defaults, an optional `config.toml`, and env
//! overrides. Resolution is env over file over default (see
//! docs/adr/0004-layered-config-env-over-file.md). A partial TOML file layers
//! over the defaults via `#[serde(default)]`; env vars layer on last.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A search-link template. `{query}` is replaced with the cleaned, encoded
/// folder name when a row renders. Parsed now so the schema is stable; the
/// rendering is wired in a later increment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SearchLink {
    /// Text shown on the link button.
    pub label: String,
    /// URL template; `{query}` is replaced with the encoded folder name.
    pub url: String,
}

/// The fully resolved configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Library roots to scan. Each is rendered as its own tree.
    pub library_roots: Vec<PathBuf>,
    /// Address the HTTP server binds to.
    pub bind: String,
    /// HTTP listen port.
    pub port: u16,
    /// Scan-cache staleness backstop, in seconds.
    pub ttl_seconds: u64,
    /// Audio extensions counted as audio, compared case-insensitively.
    pub audio_exts: Vec<String>,
    /// Ebook extensions counted as coverage, compared case-insensitively.
    pub ebook_exts: Vec<String>,
    /// Exact directory names pruned anywhere in the tree (case-insensitive).
    pub excluded_dirs: Vec<String>,
    /// Glob patterns matched against each folder's root-relative path.
    pub exclude_globs: Vec<String>,
    /// Search-link templates rendered beside each flagged folder.
    pub search_links: Vec<SearchLink>,
}

impl Default for Config {
    fn default() -> Self {
        let strings =
            |items: &[&str]| -> Vec<String> { items.iter().map(|s| s.to_string()).collect() };
        Self {
            library_roots: Vec::new(),
            bind: "127.0.0.1".to_string(),
            port: 8080,
            ttl_seconds: 60,
            // Audiobookshelf's full supported sets; see ADR-0006.
            audio_exts: strings(&[
                ".m4b", ".mp3", ".m4a", ".flac", ".opus", ".ogg", ".oga", ".mp4", ".aac", ".wma",
                ".aiff", ".aif", ".wav", ".webm", ".webma", ".mka", ".awb", ".caf", ".mpg",
                ".mpeg",
            ]),
            ebook_exts: strings(&[".epub", ".pdf", ".mobi", ".azw3", ".cbr", ".cbz"]),
            excluded_dirs: Vec::new(),
            exclude_globs: Vec::new(),
            search_links: vec![
                SearchLink {
                    label: "Goodreads".to_string(),
                    url: "https://www.goodreads.com/search?q={query}".to_string(),
                },
                SearchLink {
                    label: "OceanofPDF".to_string(),
                    url: "https://oceanofpdf.com/?s={query}".to_string(),
                },
            ],
        }
    }
}

/// Failures while resolving config.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config file could not be read from disk.
    #[error("could not read config file {path}: {source}")]
    Read {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying IO error.
        source: std::io::Error,
    },
    /// The config file is not valid TOML, or it carries an unknown key.
    #[error("could not parse config file {path}: {source}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying TOML parse error.
        source: toml::de::Error,
    },
    /// No library roots were set in any layer.
    #[error(
        "no library roots configured. Set MISSING_EBOOKS_LIBRARY_ROOTS or add \
         `library_roots` to config.toml (run with --print-config for a template)."
    )]
    MissingLibraryRoots,
}

impl Config {
    /// Resolve config: defaults, then an optional file, then env overrides,
    /// then validate that at least one library root is set.
    pub fn load(config_path: Option<&Path>) -> Result<Config, ConfigError> {
        let mut cfg = match config_path {
            Some(path) => Self::from_file(path)?,
            None => Config::default(),
        };
        apply_env_overrides(&mut cfg, &|key| std::env::var(key).ok());
        cfg.validate()?;
        Ok(cfg)
    }

    fn from_file(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.library_roots.is_empty() {
            return Err(ConfigError::MissingLibraryRoots);
        }
        Ok(())
    }
}

/// Layer environment variables over `cfg`. The getter is injected so tests can
/// drive it without touching the real process environment.
fn apply_env_overrides(cfg: &mut Config, getenv: &dyn Fn(&str) -> Option<String>) {
    if let Some(raw) = getenv("MISSING_EBOOKS_LIBRARY_ROOTS") {
        cfg.library_roots = std::env::split_paths(&raw).collect();
    }
    if let Some(bind) = getenv("MISSING_EBOOKS_BIND") {
        cfg.bind = bind;
    }
    if let Some(port) = getenv("MISSING_EBOOKS_PORT").and_then(|v| v.parse().ok()) {
        cfg.port = port;
    }
    if let Some(ttl) = getenv("MISSING_EBOOKS_TTL_SECONDS").and_then(|v| v.parse().ok()) {
        cfg.ttl_seconds = ttl;
    }
}

/// Return the commented `config.toml` template that `--print-config` emits.
pub fn print_config_template() -> &'static str {
    CONFIG_TEMPLATE
}

/// The commented template. It must stay parseable into `Config`;
/// `print_config_template_round_trips` guards against drift.
pub const CONFIG_TEMPLATE: &str = r##"# One or more library roots. Each is scanned and rendered as its own tree.
# Required: the server exits if this is unset in every layer. Also settable as
# MISSING_EBOOKS_LIBRARY_ROOTS.
library_roots = []
# Example: library_roots = ["/mnt/jane-nas/Entertainment/Audiobooks"]

# Address the HTTP server binds. Loopback by default (see ADR-0003). Set
# "0.0.0.0" to listen on all interfaces; the server logs a warning at startup
# when bound to a non-loopback address. Also settable as MISSING_EBOOKS_BIND.
bind = "127.0.0.1"

# HTTP listen port. Also settable as MISSING_EBOOKS_PORT.
port = 8080

# Scan-cache staleness backstop in seconds. When a request arrives on a cache
# older than this, the server rescans before responding. /rescan is the primary
# freshness control. Also settable as MISSING_EBOOKS_TTL_SECONDS.
ttl_seconds = 60

# File extensions, compared case-insensitively. Leading dot required. The
# defaults mirror Audiobookshelf's full supported sets (see ADR-0006).
audio_exts = [".m4b", ".mp3", ".m4a", ".flac", ".opus", ".ogg", ".oga", ".mp4", ".aac", ".wma", ".aiff", ".aif", ".wav", ".webm", ".webma", ".mka", ".awb", ".caf", ".mpg", ".mpeg"]
ebook_exts = [".epub", ".pdf", ".mobi", ".azw3", ".cbr", ".cbz"]

# Marker files are not configurable. The two fixed names .no_ebook and
# .ebook_elsewhere mark a folder as covered; both are used for detection and the
# write buttons, so they can never drift apart.

# Exact directory names to exclude (case-insensitive), applied anywhere in the
# tree. A match drops the folder and its whole subtree. Dot-prefixed entries such
# as .DS_Store and .@__thumb need no entry: any file or directory whose name
# starts with a dot is skipped automatically, matching Audiobookshelf.
excluded_dirs = []
# Example: excluded_dirs = ["@eaDir", "#recycle"]

# Glob patterns matched against the folder path relative to its library root. A
# match drops the folder and its whole subtree (see ADR-0001).
exclude_globs = []
# Example: exclude_globs = ["**/*(abridged)*", "**/*(Dramatized Adaptation)*"]

# Search-link templates. {query} is replaced with the cleaned, URL-encoded
# folder name.
[[search_links]]
label = "Goodreads"
url = "https://www.goodreads.com/search?q={query}"

[[search_links]]
label = "OceanofPDF"
url = "https://oceanofpdf.com/?s={query}"

# Shadow-library mirrors rotate their domains. Update these to a live mirror
# before uncommenting.
# [[search_links]]
# label = "Anna's Archive"
# url = "https://annas-archive.gl/search?q={query}"
#
# [[search_links]]
# label = "Library Genesis"
# url = "https://libgen.is/search.php?req={query}"
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_match_the_documented_schema() {
        let cfg = Config::default();
        assert_eq!(cfg.bind, "127.0.0.1");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.ttl_seconds, 60);
        assert_eq!(cfg.audio_exts.len(), 20); // ABS's full audio set; see ADR-0006
        assert!(cfg.audio_exts.contains(&".mp3".to_string()));
        assert!(cfg.audio_exts.contains(&".opus".to_string()));
        assert_eq!(
            cfg.ebook_exts,
            vec![".epub", ".pdf", ".mobi", ".azw3", ".cbr", ".cbz"]
        );
        assert!(cfg.library_roots.is_empty());
        assert!(cfg.excluded_dirs.is_empty());
        assert!(cfg.exclude_globs.is_empty());
        let labels: Vec<&str> = cfg.search_links.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, vec!["Goodreads", "OceanofPDF"]);
    }

    #[test]
    fn file_overrides_defaults_per_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "library_roots = [\"/tmp/lib\"]\nport = 9000\n").unwrap();

        let cfg = Config::from_file(&path).unwrap();
        assert_eq!(cfg.port, 9000); // from the file
        assert_eq!(cfg.bind, "127.0.0.1"); // untouched field keeps the default
        assert_eq!(cfg.audio_exts.len(), 20); // default audio set is untouched
        assert_eq!(
            cfg.library_roots,
            vec![std::path::PathBuf::from("/tmp/lib")]
        );
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "nonsense = true\n").unwrap();
        assert!(matches!(
            Config::from_file(&path),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn env_overrides_scalar_fields() {
        let mut cfg = Config::default();
        let env = fake_env(&[
            ("MISSING_EBOOKS_PORT", "1234"),
            ("MISSING_EBOOKS_BIND", "0.0.0.0"),
        ]);
        apply_env_overrides(&mut cfg, &|k| env.get(k).cloned());
        assert_eq!(cfg.port, 1234);
        assert_eq!(cfg.bind, "0.0.0.0");
        assert_eq!(cfg.ttl_seconds, 60); // unset env leaves the default
    }

    #[test]
    fn env_library_roots_split_on_the_platform_separator() {
        let joined =
            std::env::join_paths([std::path::Path::new("/a/b"), std::path::Path::new("/c/d")])
                .unwrap()
                .into_string()
                .unwrap();
        let mut cfg = Config::default();
        let env = fake_env(&[("MISSING_EBOOKS_LIBRARY_ROOTS", &joined)]);
        apply_env_overrides(&mut cfg, &|k| env.get(k).cloned());
        assert_eq!(
            cfg.library_roots,
            vec![
                std::path::PathBuf::from("/a/b"),
                std::path::PathBuf::from("/c/d")
            ]
        );
    }

    #[test]
    fn missing_library_roots_is_an_error() {
        assert!(matches!(
            Config::default().validate(),
            Err(ConfigError::MissingLibraryRoots)
        ));
        let mut cfg = Config::default();
        cfg.library_roots.push(std::path::PathBuf::from("/tmp/lib"));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn print_config_template_round_trips() {
        let parsed: Config = toml::from_str(CONFIG_TEMPLATE).expect("template must parse");
        assert_eq!(parsed.port, 8080);
        assert_eq!(parsed.bind, "127.0.0.1");
        assert_eq!(parsed.audio_exts.len(), 20);
        assert!(parsed.audio_exts.contains(&".opus".to_string()));
        let labels: Vec<&str> = parsed
            .search_links
            .iter()
            .map(|l| l.label.as_str())
            .collect();
        assert_eq!(labels, vec!["Goodreads", "OceanofPDF"]);
    }
}
