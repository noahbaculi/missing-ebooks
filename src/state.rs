//! Application state: the immutable `Arc<Config>` and compiled `Arc<ScanSettings>`,
//! plus a TTL-memoized scan cache behind one mutex. The cache is read once at
//! startup and never written back (see docs/adr/0002-v1-runtime-write-model.md).

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::Config;
use crate::scanner::ScanSettings;
use crate::service::FlaggedView;

/// Everything a request handler needs: the immutable config and settings, and
/// the scan cache. Shared as `Arc<AppState>`.
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) settings: Arc<ScanSettings>,
    pub(crate) cache: Cache,
}

/// The scan cache: one slot guarded by a mutex, plus the staleness window. A
/// `None` TTL disables caching (every read rescans).
pub(crate) struct Cache {
    pub(crate) entry: Mutex<Option<CacheEntry>>,
    pub(crate) ttl: Option<Duration>,
}

/// A stored view and the instant it was scanned.
pub(crate) struct CacheEntry {
    pub(crate) stored_at: Instant,
    pub(crate) view: Arc<FlaggedView>,
}

impl AppState {
    /// Build the shared state. `ttl_seconds == 0` disables the cache; any other
    /// value sets the staleness window.
    pub fn new(config: Config, settings: ScanSettings) -> AppState {
        let ttl = if config.ttl_seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(config.ttl_seconds))
        };
        AppState {
            config: Arc::new(config),
            settings: Arc::new(settings),
            cache: Cache {
                entry: Mutex::new(None),
                ttl,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ScanSettings {
        let defaults = Config::default();
        ScanSettings::compile(crate::scanner::ScanInputs {
            audio_exts: &defaults.audio_exts,
            ebook_exts: &defaults.ebook_exts,
            excluded_dirs: &[],
            exclude_globs: &[],
        })
        .unwrap()
    }

    #[test]
    fn ttl_zero_disables_the_cache() {
        let cfg = Config {
            ttl_seconds: 0,
            ..Default::default()
        };
        let state = AppState::new(cfg, settings());
        assert_eq!(state.cache.ttl, None);
    }

    #[test]
    fn nonzero_ttl_sets_the_window() {
        let cfg = Config {
            ttl_seconds: 90,
            ..Default::default()
        };
        let state = AppState::new(cfg, settings());
        assert_eq!(state.cache.ttl, Some(Duration::from_secs(90)));
    }
}
