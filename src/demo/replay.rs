//! Replay a session's marks against the shared base raw view. Folds
//! `apply_mark_raw`, the same rule the production write path uses, over the
//! session's `BTreeSet` of marks. Sorted iteration keeps `cover_files` byte
//! stable across renders.

use std::collections::BTreeSet;

use crate::raw_view::{RawView, apply_mark_raw};

use super::session::MarkKey;

/// Replay `marks` against `base` and return the synthesized `RawView`.
pub(super) fn replay_marks(base: &RawView, marks: &BTreeSet<MarkKey>) -> RawView {
    let mut raw = base.clone();
    for (root, rel, kind) in marks {
        apply_mark_raw(&mut raw, *root, rel, *kind);
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::marker::Marker;
    use crate::scanner::ScanSettings;
    use crate::scenarios;
    use crate::state::AppState;
    use crate::tree::ViewMode;
    use crate::web::render;
    use std::sync::Arc;

    async fn base_view() -> RawView {
        let dir = tempfile::tempdir().unwrap();
        let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
        let roots = scenarios::materialize(&(scenario.spec)(), dir.path());
        let config = Config {
            library_roots: roots,
            scan_cache_ttl_seconds: 600,
            ..Config::default()
        };
        let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
        let state = Arc::new(AppState::new(config, settings));
        (*state.store.current().await).clone()
    }

    fn render_gaps(raw: &RawView) -> String {
        render::page(raw, &[], ViewMode::GapsOnly, 0).into_string()
    }

    #[tokio::test]
    async fn no_marks_renders_the_same_as_base() {
        let base = base_view().await;
        let empty: BTreeSet<MarkKey> = BTreeSet::new();
        let replayed = replay_marks(&base, &empty);
        assert_eq!(render_gaps(&base), render_gaps(&replayed));
    }

    #[tokio::test]
    async fn replay_is_deterministic_across_calls() {
        let base = base_view().await;
        let mut marks: BTreeSet<MarkKey> = BTreeSet::new();
        marks.insert((0, "Author/Book".to_string(), Marker::NoEbook));
        marks.insert((0, "Author/Book".to_string(), Marker::EbookElsewhere));
        let a = replay_marks(&base, &marks);
        let b = replay_marks(&base, &marks);
        assert_eq!(render_gaps(&a), render_gaps(&b));
    }
}
