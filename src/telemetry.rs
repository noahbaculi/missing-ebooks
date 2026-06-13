//! Tracing subscriber setup and the verbosity-resolution logic.
//!
//! Verbosity is read from the environment at init time, before `Config` exists,
//! because the subscriber is installed before config loads (so config errors can
//! be logged). `RUST_LOG` wins outright for developers who want full filter
//! syntax. Otherwise `MISSING_EBOOKS_LOG=<level>` sets verbosity: `debug` and
//! `trace` raise only this crate over an `info` baseline, so dependency internals
//! stay quiet; `warn` and `error` apply everywhere, so dependencies are never
//! louder than the app. An unknown level falls back to `info`, the default.

use tracing_subscriber::EnvFilter;

/// The levels `MISSING_EBOOKS_LOG` accepts, ordered least to most verbose.
const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

/// Install the global tracing subscriber: human-readable events to stderr, with
/// verbosity from the environment (see [`resolve`]). Idempotent via `try_init`, so
/// a second call (a test, an example) is a no-op, not a panic.
pub fn init() {
    let resolved = resolve(&|key| std::env::var(key).ok());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&resolved.directive))
        .try_init()
        .ok();
    // Subscriber is live now, so the warning routes through it, not bare stderr.
    if let Some(value) = resolved.unknown_level {
        tracing::warn!(
            %value,
            "MISSING_EBOOKS_LOG is not a known level (error, warn, info, debug, trace); using info"
        );
    }
}

/// The resolved env-filter directive, plus the raw `MISSING_EBOOKS_LOG` value when
/// it named no known level, so [`init`] can flag it.
struct Resolution {
    directive: String,
    unknown_level: Option<String>,
}

/// Resolve the env-filter directive from the two environment knobs. The getter is
/// injected so tests drive it without touching the process environment. An empty
/// value counts as unset.
fn resolve(getenv: &dyn Fn(&str) -> Option<String>) -> Resolution {
    if let Some(rust_log) = getenv("RUST_LOG").filter(|s| !s.is_empty()) {
        return Resolution {
            directive: rust_log,
            unknown_level: None,
        };
    }
    let Some(level) = getenv("MISSING_EBOOKS_LOG").filter(|s| !s.is_empty()) else {
        return Resolution {
            directive: "info".to_string(),
            unknown_level: None,
        };
    };
    if !LEVELS.contains(&level.as_str()) {
        return Resolution {
            directive: "info".to_string(),
            unknown_level: Some(level),
        };
    }
    Resolution {
        directive: scoped_directive(&level),
        unknown_level: None,
    }
}

/// Build the directive for a known level. `debug`/`trace` pin dependencies at the
/// `info` baseline and raise only this crate; `info` and below apply one level
/// everywhere, so dependencies are never louder than the app.
fn scoped_directive(level: &str) -> String {
    match level {
        "debug" | "trace" => format!("info,missing_ebooks={level}"),
        _ => level.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn rust_log_wins_and_passes_through_unchanged() {
        let getenv = env(&[
            ("RUST_LOG", "missing_ebooks::scanner=trace"),
            ("MISSING_EBOOKS_LOG", "debug"),
        ]);
        let resolved = resolve(&getenv);
        assert_eq!(resolved.directive, "missing_ebooks::scanner=trace");
        assert_eq!(resolved.unknown_level, None);
    }

    #[test]
    fn debug_and_trace_raise_only_this_crate() {
        assert_eq!(
            resolve(&env(&[("MISSING_EBOOKS_LOG", "debug")])).directive,
            "info,missing_ebooks=debug"
        );
        assert_eq!(
            resolve(&env(&[("MISSING_EBOOKS_LOG", "trace")])).directive,
            "info,missing_ebooks=trace"
        );
    }

    #[test]
    fn warn_and_error_apply_to_everything() {
        // Lowering the level must quiet dependencies too, not leave them at info.
        assert_eq!(
            resolve(&env(&[("MISSING_EBOOKS_LOG", "warn")])).directive,
            "warn"
        );
        assert_eq!(
            resolve(&env(&[("MISSING_EBOOKS_LOG", "error")])).directive,
            "error"
        );
    }

    #[test]
    fn both_unset_defaults_to_info() {
        let resolved = resolve(&env(&[]));
        assert_eq!(resolved.directive, "info");
        assert_eq!(resolved.unknown_level, None);
    }

    #[test]
    fn empty_values_are_treated_as_unset() {
        let resolved = resolve(&env(&[("RUST_LOG", ""), ("MISSING_EBOOKS_LOG", "")]));
        assert_eq!(resolved.directive, "info");
    }

    #[test]
    fn unknown_level_falls_back_to_info_and_is_flagged() {
        let resolved = resolve(&env(&[("MISSING_EBOOKS_LOG", "debg")]));
        assert_eq!(resolved.directive, "info");
        assert_eq!(resolved.unknown_level.as_deref(), Some("debg"));
    }

    #[test]
    fn every_known_level_builds_a_valid_filter() {
        for level in LEVELS {
            let resolved = resolve(&env(&[("MISSING_EBOOKS_LOG", level)]));
            assert!(
                EnvFilter::try_new(&resolved.directive).is_ok(),
                "level {level} produced an unparsable directive: {}",
                resolved.directive
            );
        }
    }
}
