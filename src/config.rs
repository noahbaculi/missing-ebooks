//! Configuration: built-in defaults, an optional `config.toml`, and env
//! overrides. Resolution is env over file over default (see
//! docs/adr/0004-layered-config-env-over-file.md). A partial TOML file layers
//! over the defaults via `#[serde(default)]`. Env vars layer on last.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::scanner::ScanInputs;

/// A search-link template. `{query}` is replaced with the cleaned, encoded
/// folder name when a row renders (see ADR 0010).
#[derive(Debug, Clone, Deserialize)]
pub struct SearchLink {
    /// Text shown on the link button.
    pub label: String,
    /// URL template. `{query}` is replaced with the encoded folder name.
    pub url: String,
}

/// The fully resolved configuration.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Library roots to scan. Each is rendered as its own tree.
    pub library_roots: Vec<PathBuf>,
    /// Address the HTTP server binds to.
    pub bind: String,
    /// HTTP listen port.
    pub port: u16,
    /// Scan-cache staleness ceiling, in seconds. Warm reads (page loads,
    /// `/refresh` polls) serve from cache while it is younger than this and
    /// force a rebuild otherwise. Together with the client poll interval it
    /// caps how often the underlying scan runs regardless of open-tab count.
    pub ttl_seconds: u64,
    /// Directories read at once during a scan. Sizes the scan thread pool. On a
    /// network mount each directory is a round trip, so reading several at once
    /// overlaps the waits. Size it by the mount speed, not the CPU count. One
    /// pool serves the whole process, so concurrent scans share it rather than
    /// each getting this many readers.
    pub scan_concurrency: usize,
    /// Client-side poll cadence. When `> 0`, the page shell emits a
    /// `<div id="poll-root">` marker with this value; the client hits
    /// `/refresh?view=...` on that interval while the tab is visible. `0`
    /// still emits the marker (so the client can choose not to poll without a
    /// server-side branch) but suppresses the interval. See ADR-0034.
    pub poll_interval_seconds: u64,
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
            |items: &[&str]| -> Vec<String> { items.iter().map(ToString::to_string).collect() };
        Self {
            library_roots: Vec::new(),
            bind: "127.0.0.1".to_string(),
            port: 13379,
            ttl_seconds: 10,
            scan_concurrency: 16,
            poll_interval_seconds: 10,
            // Audiobookshelf's full supported sets (see ADR-0006).
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
    /// An environment variable was set but its value did not parse.
    #[error("environment variable {var}={value:?} is invalid: {source}")]
    InvalidEnv {
        /// Variable name (e.g. "MISSING_EBOOKS_PORT").
        var: String,
        /// The raw value that failed to parse.
        value: String,
        /// Underlying parse error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl Config {
    /// Resolve config: defaults, then an optional file, then env overrides,
    /// then validate that at least one library root is set.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] if the optional file cannot be read or parsed,
    /// an env override is malformed, or validation fails (e.g. no library roots
    /// are configured).
    pub fn load(config_path: Option<&Path>) -> Result<Config, ConfigError> {
        let mut cfg = match config_path {
            Some(path) => Self::from_file(path)?,
            None => Config::default(),
        };
        apply_env_overrides(&mut cfg, &|key| std::env::var(key).ok())?;
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

    /// The four list fields a scan reads, borrowed as `ScanInputs`. The mapping
    /// lives here so the scanner stays config-agnostic: the dependency points
    /// from config to scanner, never the other way.
    pub fn scan_inputs(&self) -> ScanInputs<'_> {
        ScanInputs {
            audio_exts: &self.audio_exts,
            ebook_exts: &self.ebook_exts,
            excluded_dirs: &self.excluded_dirs,
            exclude_globs: &self.exclude_globs,
        }
    }
}

/// Parse an environment variable value with a typed `FromStr`. Returns
/// `Ok(None)` when the variable is unset, and `ConfigError::InvalidEnv`
/// when it is set but does not parse. Empty string counts as set and fails
/// to parse, which is the intended behavior: an empty env value is operator
/// error, not a request to fall back.
fn parse_env<T: std::str::FromStr>(
    name: &str,
    raw: Option<String>,
) -> Result<Option<T>, ConfigError>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let Some(value) = raw else {
        return Ok(None);
    };
    value
        .parse()
        .map(Some)
        .map_err(|err: T::Err| ConfigError::InvalidEnv {
            var: name.to_string(),
            value,
            source: Box::new(err),
        })
}

/// Layer environment variables over `cfg`. The getter is injected so tests can
/// drive it without touching the real process environment.
fn apply_env_overrides(
    cfg: &mut Config,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    if let Some(raw) = getenv("MISSING_EBOOKS_LIBRARY_ROOTS") {
        cfg.library_roots = std::env::split_paths(&raw).collect();
    }
    if let Some(bind) = getenv("MISSING_EBOOKS_BIND") {
        cfg.bind = bind;
    }
    if let Some(port) = parse_env::<u16>("MISSING_EBOOKS_PORT", getenv("MISSING_EBOOKS_PORT"))? {
        cfg.port = port;
    }
    if let Some(ttl) = parse_env::<u64>(
        "MISSING_EBOOKS_TTL_SECONDS",
        getenv("MISSING_EBOOKS_TTL_SECONDS"),
    )? {
        cfg.ttl_seconds = ttl;
    }
    if let Some(n) = parse_env::<usize>(
        "MISSING_EBOOKS_SCAN_CONCURRENCY",
        getenv("MISSING_EBOOKS_SCAN_CONCURRENCY"),
    )? {
        cfg.scan_concurrency = n;
    }
    if let Some(v) = parse_env::<u64>(
        "MISSING_EBOOKS_POLL_INTERVAL_SECONDS",
        getenv("MISSING_EBOOKS_POLL_INTERVAL_SECONDS"),
    )? {
        cfg.poll_interval_seconds = v;
    }
    Ok(())
}

/// The commented template. It must stay parseable into `Config`.
/// `print_config_template_round_trips` guards against drift.
pub const CONFIG_TEMPLATE: &str = r##"# One or more library roots. Each is scanned and rendered as its own tree.
# Required: the server exits if this is unset in every layer. Also settable as
# MISSING_EBOOKS_LIBRARY_ROOTS.
library_roots = []
# Example: library_roots = ["/mnt/jane-nas/Entertainment/Audiobooks"]

# Logging is set with the MISSING_EBOOKS_LOG environment variable, not in this
# file: error, warn, info (default), debug, or trace. debug adds per-operation
# timings (scans, cache, marker writes, requests). trace adds a line per
# directory. RUST_LOG, if set, overrides it with full tracing filter syntax.

# Address the HTTP server binds. Loopback by default (see ADR-0003). Set
# "0.0.0.0" to listen on all interfaces. The server logs a warning at startup
# when bound to a non-loopback address. Also settable as MISSING_EBOOKS_BIND.
bind = "127.0.0.1"

# HTTP listen port. An uncommon high port, away from 8080 (see ADR-0011). Also
# settable as MISSING_EBOOKS_PORT.
port = 13379

# Scan-cache staleness ceiling in seconds. Warm reads (page loads, /refresh
# polls) serve from cache while it is younger than this and force a rebuild
# otherwise. Together with poll_interval_seconds it caps how often the
# underlying scan runs regardless of open-tab count. 0 disables the cache and
# rescans on every request. /rescan is the primary freshness control for a
# user who wants to know now. Also settable as MISSING_EBOOKS_TTL_SECONDS.
ttl_seconds = 10

# Directories the library scan reads at once. The scan is bound by per-directory
# latency on a network mount (SMB/NFS), where each folder is a round trip, so
# reading several at once overlaps the waits. Size this by the speed of the
# mount, not the CPU count: the threads mostly wait on the network. One pool
# serves the whole process, so concurrent scans share it. 1 disables the
# parallelism. Also settable as MISSING_EBOOKS_SCAN_CONCURRENCY.
scan_concurrency = 16

# Client-side poll cadence. When > 0, open tabs pull /refresh every N seconds
# while the tab is visible, and ttl_seconds caps how often the underlying scan
# actually runs regardless of open-tab count. 0 keeps the poll marker in the
# page but suppresses the interval so the client stays quiet. Also settable as
# MISSING_EBOOKS_POLL_INTERVAL_SECONDS.
poll_interval_seconds = 10

# File extensions, compared case-insensitively. Leading dot required. The
# defaults mirror Audiobookshelf's full supported sets (see ADR-0006).
audio_exts = [".m4b", ".mp3", ".m4a", ".flac", ".opus", ".ogg", ".oga", ".mp4", ".aac", ".wma", ".aiff", ".aif", ".wav", ".webm", ".webma", ".mka", ".awb", ".caf", ".mpg", ".mpeg"]
ebook_exts = [".epub", ".pdf", ".mobi", ".azw3", ".cbr", ".cbz"]

# Marker files are not configurable. The two fixed names .no_ebook and
# .ebook_elsewhere mark a folder as covered. Both are used for detection and the
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
        assert_eq!(cfg.port, 13379);
        assert_eq!(cfg.ttl_seconds, 10);
        assert_eq!(cfg.audio_exts.len(), 20); // ABS's full audio set (ADR-0006)
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
        assert_eq!(cfg.scan_concurrency, 16);
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
        apply_env_overrides(&mut cfg, &|k| env.get(k).cloned()).unwrap();
        assert_eq!(cfg.port, 1234);
        assert_eq!(cfg.bind, "0.0.0.0");
        assert_eq!(cfg.ttl_seconds, 10); // unset env leaves the default
    }

    #[test]
    fn env_overrides_scan_concurrency() {
        let mut cfg = Config::default();
        let env = fake_env(&[("MISSING_EBOOKS_SCAN_CONCURRENCY", "32")]);
        apply_env_overrides(&mut cfg, &|k| env.get(k).cloned()).unwrap();
        assert_eq!(cfg.scan_concurrency, 32);
    }

    #[test]
    fn defaults_pin_the_client_poll_shape() {
        let cfg = Config::default();
        assert_eq!(cfg.poll_interval_seconds, 10);
        assert_eq!(cfg.ttl_seconds, 10);
    }

    #[test]
    fn env_overrides_poll_interval_seconds() {
        let mut cfg = Config::default();
        let env = fake_env(&[("MISSING_EBOOKS_POLL_INTERVAL_SECONDS", "5")]);
        apply_env_overrides(&mut cfg, &|k| env.get(k).cloned()).unwrap();
        assert_eq!(cfg.poll_interval_seconds, 5);
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
        apply_env_overrides(&mut cfg, &|k| env.get(k).cloned()).unwrap();
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
    fn scan_inputs_borrows_the_four_scan_fields() {
        let cfg = Config::default();
        let inputs = cfg.scan_inputs();
        assert_eq!(inputs.audio_exts, cfg.audio_exts.as_slice());
        assert_eq!(inputs.ebook_exts, cfg.ebook_exts.as_slice());
        assert_eq!(inputs.excluded_dirs, cfg.excluded_dirs.as_slice());
        assert_eq!(inputs.exclude_globs, cfg.exclude_globs.as_slice());
    }

    #[test]
    fn env_invalid_port_fails_with_named_variable_error() {
        let mut cfg = Config::default();
        let env = fake_env(&[("MISSING_EBOOKS_PORT", "garbage")]);
        let err = apply_env_overrides(&mut cfg, &|k| env.get(k).cloned())
            .expect_err("invalid env value must error");
        match err {
            ConfigError::InvalidEnv { var, value, .. } => {
                assert_eq!(var, "MISSING_EBOOKS_PORT");
                assert_eq!(value, "garbage");
            }
            other => panic!("expected InvalidEnv, got {other:?}"),
        }
    }

    #[test]
    fn env_empty_value_fails() {
        let mut cfg = Config::default();
        let env = fake_env(&[("MISSING_EBOOKS_TTL_SECONDS", "")]);
        let err = apply_env_overrides(&mut cfg, &|k| env.get(k).cloned())
            .expect_err("empty env value must error");
        assert!(
            matches!(err, ConfigError::InvalidEnv { ref var, .. } if var == "MISSING_EBOOKS_TTL_SECONDS")
        );
    }

    #[test]
    fn env_valid_value_still_applies() {
        // Regression guard: the happy path is unchanged by the new signature.
        let mut cfg = Config::default();
        let env = fake_env(&[
            ("MISSING_EBOOKS_PORT", "9000"),
            ("MISSING_EBOOKS_TTL_SECONDS", "120"),
        ]);
        apply_env_overrides(&mut cfg, &|k| env.get(k).cloned()).unwrap();
        assert_eq!(cfg.port, 9000);
        assert_eq!(cfg.ttl_seconds, 120);
    }

    #[test]
    fn print_config_template_round_trips() {
        let parsed: Config = toml::from_str(CONFIG_TEMPLATE).expect("template must parse");
        assert_eq!(parsed.port, 13379);
        assert_eq!(parsed.bind, "127.0.0.1");
        assert_eq!(parsed.audio_exts.len(), 20);
        assert!(parsed.audio_exts.contains(&".opus".to_string()));
        let labels: Vec<&str> = parsed
            .search_links
            .iter()
            .map(|l| l.label.as_str())
            .collect();
        assert_eq!(labels, vec!["Goodreads", "OceanofPDF"]);
        assert_eq!(parsed.scan_concurrency, 16);
    }
}
