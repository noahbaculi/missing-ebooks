//! Tracing subscriber setup and the verbosity-resolution logic.
//!
//! Verbosity is read from the environment at init time, before `Config` exists,
//! because the subscriber is installed before config loads (so config errors can
//! be logged). `RUST_LOG` wins outright for developers who want full filter
//! syntax; otherwise `MISSING_EBOOKS_LOG=<level>` raises only this crate over an
//! `info` baseline; otherwise the default is `info`.

use tracing_subscriber::EnvFilter;

/// Install the global tracing subscriber: human-readable events to stderr, with
/// verbosity from the environment (see [`filter_directive`]). Idempotent via
/// `try_init`, so a second call (a test, an example) is a no-op, not a panic.
pub fn init() {
    let directive = filter_directive(&|key| std::env::var(key).ok());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(directive))
        .try_init()
        .ok();
}

/// Resolve the env-filter directive string from the two environment knobs. The
/// getter is injected so tests drive it without touching the process environment.
/// An empty value counts as unset.
fn filter_directive(getenv: &dyn Fn(&str) -> Option<String>) -> String {
    if let Some(rust_log) = getenv("RUST_LOG").filter(|s| !s.is_empty()) {
        return rust_log;
    }
    if let Some(level) = getenv("MISSING_EBOOKS_LOG").filter(|s| !s.is_empty()) {
        return format!("info,missing_ebooks={level}");
    }
    "info".to_string()
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
        assert_eq!(filter_directive(&getenv), "missing_ebooks::scanner=trace");
    }

    #[test]
    fn missing_ebooks_log_scopes_the_level_to_this_crate() {
        let getenv = env(&[("MISSING_EBOOKS_LOG", "debug")]);
        assert_eq!(filter_directive(&getenv), "info,missing_ebooks=debug");
    }

    #[test]
    fn both_unset_defaults_to_info() {
        let getenv = env(&[]);
        assert_eq!(filter_directive(&getenv), "info");
    }

    #[test]
    fn empty_values_are_treated_as_unset() {
        let getenv = env(&[("RUST_LOG", ""), ("MISSING_EBOOKS_LOG", "")]);
        assert_eq!(filter_directive(&getenv), "info");
    }
}
