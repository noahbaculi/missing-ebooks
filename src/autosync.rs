//! Background autosync. The `Autosync` struct on `AppState` owns a subscriber
//! registry and a single loop task; the loop wakes every
//! `autosync_interval_seconds` while at least one SSE client is connected,
//! diffs the rendered sections against the last broadcast, and pushes OOB
//! swap fragments for the ones that changed. See ADR-0023, ADR-0024.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use enum_map::EnumMap;

use crate::service::ViewMode;
use crate::state;

/// Diff each rendered section against `last_hash` and return the list of pushes
/// to fan out, mutating `last_hash` in place to reflect what is about to be
/// sent. The `render` closure produces the HTML for a `(mode, root)` pair; the
/// loop wires the real renderer, tests pass a sentinel.
///
/// `has_subs[mode]` short-circuits modes nobody is listening to: their hashes
/// stay untouched and they produce no pushes.
#[allow(dead_code, reason = "wired up by the autosync loop in Task 6")]
pub(crate) fn compute_pushes<R>(
    raw: &state::RawView,
    last_hash: &mut EnumMap<ViewMode, Vec<Option<u64>>>,
    has_subs: EnumMap<ViewMode, bool>,
    mut render: R,
) -> Vec<(ViewMode, usize, String)>
where
    R: FnMut(ViewMode, usize, &state::RawRootSection) -> String,
{
    let mut pushes = Vec::new();
    for mode in [ViewMode::GapsOnly, ViewMode::All] {
        if !has_subs[mode] {
            continue;
        }
        // Resize the per-mode hash vec to match the current root count: roots
        // are config-fixed for a process, but the first call has empty vecs.
        if last_hash[mode].len() != raw.len() {
            last_hash[mode].resize(raw.len(), None);
        }
        for (root_idx, section) in raw.iter().enumerate() {
            let html = render(mode, root_idx, section);
            let h = stable_hash(&html);
            if last_hash[mode][root_idx] != Some(h) {
                last_hash[mode][root_idx] = Some(h);
                pushes.push((mode, root_idx, html));
            }
        }
    }
    pushes
}

/// A fixed seed-less hash of the rendered HTML. `DefaultHasher` is fine because
/// the comparison is equal/not-equal within one process; the hash never crosses
/// a process boundary and never seeds a security-sensitive table.
#[allow(dead_code, reason = "wired up by the autosync loop in Task 6")]
fn stable_hash(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{RawRootSection, RawRootState};
    use enum_map::enum_map;

    fn empty_raw_view(n: usize) -> state::RawView {
        (0..n)
            .map(|i| RawRootSection {
                path: format!("/root/{i}"),
                state: RawRootState::Clean,
            })
            .collect()
    }

    fn both_modes_subscribed() -> EnumMap<ViewMode, bool> {
        enum_map! { ViewMode::GapsOnly => true, ViewMode::All => true }
    }

    fn empty_hashes() -> EnumMap<ViewMode, Vec<Option<u64>>> {
        enum_map! { ViewMode::GapsOnly => Vec::new(), ViewMode::All => Vec::new() }
    }

    #[test]
    fn first_call_pushes_every_mode_root_pair() {
        let raw = empty_raw_view(2);
        let mut hashes = empty_hashes();
        let pushes = compute_pushes(
            &raw,
            &mut hashes,
            both_modes_subscribed(),
            |mode, root, _| format!("{mode:?}-{root}"),
        );
        assert_eq!(pushes.len(), 4, "two modes times two roots");
        assert!(pushes.iter().all(|(_, _, html)| !html.is_empty()));
    }

    #[test]
    fn identical_second_call_pushes_nothing() {
        let raw = empty_raw_view(2);
        let mut hashes = empty_hashes();
        let render = |mode: ViewMode, root: usize, _: &RawRootSection| format!("{mode:?}-{root}");
        let _first = compute_pushes(&raw, &mut hashes, both_modes_subscribed(), render);
        let second = compute_pushes(&raw, &mut hashes, both_modes_subscribed(), render);
        assert!(second.is_empty(), "no roots changed, no pushes");
    }

    #[test]
    fn changed_root_produces_exactly_one_push_and_touches_one_hash_slot() {
        let raw = empty_raw_view(3);
        let mut hashes = empty_hashes();
        // Seed: render every (mode, root) once, fill the hash slots.
        let _ = compute_pushes(&raw, &mut hashes, both_modes_subscribed(), |m, r, _| {
            format!("{m:?}-{r}")
        });
        let hashes_before = hashes.clone();

        // Second call: render returns a new string only for (GapsOnly, root 1).
        let pushes = compute_pushes(
            &raw,
            &mut hashes,
            both_modes_subscribed(),
            |m, r, _| match (m, r) {
                (ViewMode::GapsOnly, 1) => "MUTATED".to_string(),
                _ => format!("{m:?}-{r}"),
            },
        );
        assert_eq!(pushes.len(), 1, "exactly one (mode, root) changed");
        assert_eq!(pushes[0].0, ViewMode::GapsOnly);
        assert_eq!(pushes[0].1, 1);

        // Only that one hash slot moved; everything else equals the prior state.
        assert_ne!(
            hashes[ViewMode::GapsOnly][1],
            hashes_before[ViewMode::GapsOnly][1]
        );
        assert_eq!(
            hashes[ViewMode::GapsOnly][0],
            hashes_before[ViewMode::GapsOnly][0]
        );
        assert_eq!(
            hashes[ViewMode::GapsOnly][2],
            hashes_before[ViewMode::GapsOnly][2]
        );
        assert_eq!(hashes[ViewMode::All], hashes_before[ViewMode::All]);
    }

    #[test]
    fn mode_with_no_subscribers_is_skipped_and_its_hashes_stay_untouched() {
        let raw = empty_raw_view(2);
        let mut hashes = empty_hashes();
        let only_gaps = enum_map! { ViewMode::GapsOnly => true, ViewMode::All => false };
        let pushes = compute_pushes(&raw, &mut hashes, only_gaps, |m, r, _| format!("{m:?}-{r}"));
        assert_eq!(pushes.len(), 2, "two roots in GapsOnly only");
        assert!(pushes.iter().all(|(m, _, _)| *m == ViewMode::GapsOnly));
        assert!(
            hashes[ViewMode::All].is_empty(),
            "the All mode's hashes were never resized or written"
        );
    }
}
