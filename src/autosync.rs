//! Background autosync. The `Autosync` struct on `AppState` owns a subscriber
//! registry and a single loop task; the loop wakes every
//! `autosync_interval_seconds` while at least one SSE client is connected,
//! diffs the rendered sections against the last broadcast, and pushes OOB
//! swap fragments for the ones that changed. See ADR-0023, ADR-0024.

use std::collections::hash_map::DefaultHasher;
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use axum::response::sse::Event;
use enum_map::EnumMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

use crate::service::ViewMode;
use crate::state;

/// Diff each rendered section against `last_hash` and return the list of pushes
/// to fan out, mutating `last_hash` in place to reflect what is about to be
/// sent. The `render` closure produces the HTML for a `(mode, root)` pair; the
/// loop wires the real renderer, tests pass a sentinel.
///
/// `has_subs[mode]` short-circuits modes nobody is listening to: their hashes
/// stay untouched and they produce no pushes.
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
fn stable_hash(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Wrap one rendered section in the OOB-swap markup the autosync stream uses,
/// so the client can route the fragment to its `<section id="root-N-section">`
/// on the open page (see ADR-0024). Shared by the snapshot path and the loop's
/// per-root diff so a section's bytes are identical on both wires.
pub(crate) fn render_oob_section(
    raw_section: &state::RawRootSection,
    root_idx: usize,
    mode: ViewMode,
    links: &[crate::config::SearchLink],
) -> String {
    let rendered_state =
        crate::service::render_root_state(&raw_section.path, &raw_section.state, mode);
    let rendered_section = crate::service::RootSection {
        path: raw_section.path.clone(),
        state: rendered_state,
    };
    let inner = crate::web::render::render_section(&rendered_section, root_idx, None, links, mode)
        .into_string();
    format!("<div hx-swap-oob=\"outerHTML:#root-{root_idx}-section transition:true\">{inner}</div>")
}

/// Build the concatenated OOB-swap payload for an SSE `snapshot` event and the
/// per-root hashes the autosync loop will use to suppress redundant first-tick
/// section events. The handler sends the payload, then passes the hashes to
/// `Autosync::subscribe` so the loop's first compute_pushes finds matching
/// hashes and emits nothing until something actually changes.
pub(crate) fn snapshot_and_seed(
    raw: &state::RawView,
    mode: ViewMode,
    links: &[crate::config::SearchLink],
) -> (String, Vec<u64>) {
    let mut payload = String::with_capacity(raw.len() * 512);
    let mut hashes = Vec::with_capacity(raw.len());
    for (root_idx, section) in raw.iter().enumerate() {
        let oob = render_oob_section(section, root_idx, mode, links);
        hashes.push(stable_hash(&oob));
        payload.push_str(&oob);
    }
    (payload, hashes)
}

/// One subscriber's outbound channel. The loop fans out to every sender in
/// `subs[mode]`; a `try_send` failure prunes the sender.
pub(crate) type SseSender = mpsc::Sender<Result<Event, Infallible>>;

/// The shared autosync state for one process. Construct one per `AppState`.
/// The inner `Arc<Mutex<...>>` is cloned by the loop and by per-request handler
/// views, so the registry is one shared object regardless of how many `Autosync`
/// values point at it.
pub struct Autosync {
    inner: Arc<StdMutex<AutosyncInner>>,
    /// The configured idle gap. `0` disables the loop entirely; subscribing
    /// still works (the snapshot is sent), but no loop task is ever spawned.
    interval: Duration,
}

/// The registry behind the lock: per-mode subscribers, per-(mode, root)
/// last-broadcast hashes, and the active loop task's handle.
struct AutosyncInner {
    subs: EnumMap<ViewMode, Vec<SseSender>>,
    last_hash: EnumMap<ViewMode, Vec<Option<u64>>>,
    /// Set while the loop is running; cleared by the loop on exit.
    loop_task: Option<JoinHandle<()>>,
}

impl Autosync {
    /// Build an empty registry. The loop is not spawned until the first
    /// subscriber arrives, even when `autosync_interval_seconds > 0`.
    #[must_use]
    pub fn new(autosync_interval_seconds: u64) -> Self {
        let inner = AutosyncInner {
            subs: EnumMap::default(),
            last_hash: EnumMap::default(),
            loop_task: None,
        };
        Self {
            inner: Arc::new(StdMutex::new(inner)),
            interval: Duration::from_secs(autosync_interval_seconds),
        }
    }

    /// Register a subscriber's mpsc sender under `mode`. If the registry was
    /// empty and the interval is non-zero, spawn the loop task and store its
    /// handle. The caller is responsible for sending the snapshot event into
    /// `sender` before calling this, so the channel's first event is always
    /// the snapshot.
    ///
    /// `seed_hashes` populates `last_hash[mode]` so the loop's first compute
    /// suppresses redundant section events for sections the snapshot already
    /// carried; pass `None` (tests only) to skip the seed.
    pub(crate) fn subscribe(
        &self,
        state: &Arc<crate::state::AppState>,
        mode: ViewMode,
        sender: SseSender,
        seed_hashes: Option<Vec<u64>>,
    ) {
        let mut guard = self.inner.lock().expect("autosync mutex poisoned");
        guard.subs[mode].push(sender);
        if let Some(hashes) = seed_hashes {
            guard.last_hash[mode] = hashes.into_iter().map(Some).collect();
        }

        let should_spawn = self.interval > Duration::ZERO
            && guard
                .loop_task
                .as_ref()
                .map(JoinHandle::is_finished)
                .unwrap_or(true);
        if should_spawn {
            let weak_state = Arc::downgrade(state);
            let interval = self.interval;
            let inner = Arc::clone(&self.inner);
            let handle = tokio::spawn(async move {
                run_loop(weak_state, inner, interval).await;
            });
            guard.loop_task = Some(handle);
        }
    }

    /// Number of active subscribers across both modes. Tests reach in;
    /// production code does not.
    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        let guard = self.inner.lock().expect("autosync mutex poisoned");
        guard.subs.values().map(Vec::len).sum()
    }

    /// Abort the loop task without removing subscribers. Tests use this to
    /// simulate a panic inside the loop and confirm the next subscribe
    /// respawns.
    #[cfg(test)]
    pub(crate) fn abort_loop_for_test(&self) {
        let mut guard = self.inner.lock().expect("autosync mutex poisoned");
        if let Some(h) = guard.loop_task.take() {
            h.abort();
        }
    }
}

/// The loop body. Holds a `Weak<AppState>` so the application can drop without
/// leaking the loop; a failed upgrade per tick means the process is shutting
/// down or the test scope ended, and the loop exits.
async fn run_loop(
    weak_state: Weak<crate::state::AppState>,
    inner: Arc<StdMutex<AutosyncInner>>,
    interval: Duration,
) {
    loop {
        let Some(state) = weak_state.upgrade() else {
            tracing::debug!("autosync loop exits: app state dropped");
            return;
        };
        // Exit cleanly if every subscriber has gone away. Take the lock long
        // enough to read the count; do not hold it across the rebuild.
        let any_subs = {
            let guard = inner.lock().expect("autosync mutex poisoned");
            guard.subs.values().any(|v| !v.is_empty())
        };
        if !any_subs {
            let mut guard = inner.lock().expect("autosync mutex poisoned");
            guard.loop_task = None;
            tracing::debug!("autosync loop exits: no subscribers");
            return;
        }

        // Single-flighted with /rescan and page-load rebuilds via Cache::rebuild.
        let raw = state
            .cache
            .rebuild(|| {
                crate::service::build_view(
                    state.config.as_ref(),
                    &state.settings,
                    Arc::clone(&state.dir_index),
                )
            })
            .await;

        // Render and diff under the registry lock. The critical section is
        // short: per-section render is microseconds (ADR-0022) and there is no
        // await between lock and unlock.
        let to_send: Vec<(ViewMode, usize, String)> = {
            let mut guard = inner.lock().expect("autosync mutex poisoned");
            let has_subs = EnumMap::from_fn(|mode| !guard.subs[mode].is_empty());
            let links = &state.config.search_links;
            compute_pushes(
                &raw,
                &mut guard.last_hash,
                has_subs,
                |mode, root_idx, raw_section| {
                    render_oob_section(raw_section, root_idx, mode, links)
                },
            )
        };

        // Fan out and prune. A failed try_send drops that sender from the list.
        if !to_send.is_empty() {
            let mut guard = inner.lock().expect("autosync mutex poisoned");
            for (mode, _root_idx, html) in to_send {
                let event = section_event(html);
                guard.subs[mode].retain(|tx| tx.try_send(Ok(event.clone())).is_ok());
            }
        } else {
            // Even with no pushes, prune any senders whose receiver already
            // dropped, so the loop notices a quiet client disappearing and
            // can exit on the next iteration when the last sub goes away.
            let mut guard = inner.lock().expect("autosync mutex poisoned");
            for mode in [ViewMode::GapsOnly, ViewMode::All] {
                guard.subs[mode].retain(|tx| !tx.is_closed());
            }
        }

        sleep(interval).await;
    }
}

/// Build the SSE `section` event for one root's OOB swap fragment. The event
/// name lines up with the client's `sse-swap="section"` attribute; the OOB
/// target ID is carried inside the HTML body itself via `hx-swap-oob`.
fn section_event(html: String) -> Event {
    Event::default().event("section").data(html)
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

    use std::path::PathBuf;

    fn test_state_with_interval(autosync_interval_seconds: u64) -> Arc<crate::state::AppState> {
        let dir = tempfile::tempdir().expect("tempdir");
        let scenario = crate::scenarios::find_scenario("clean-error").expect("scenario exists");
        let roots: Vec<PathBuf> = (scenario.build)(dir.path());
        let config = crate::config::Config {
            library_roots: roots,
            ttl_seconds: 600,
            autosync_interval_seconds,
            ..crate::config::Config::default()
        };
        let settings = crate::scanner::ScanSettings::compile(config.scan_inputs()).unwrap();
        // Leak the tempdir to keep the seeded roots around for the test's
        // lifetime; the OS cleans /tmp at process exit.
        std::mem::forget(dir);
        Arc::new(crate::state::AppState::new(config, settings))
    }

    #[tokio::test]
    async fn first_subscribe_spawns_loop_last_unsub_lets_it_exit() {
        let state = test_state_with_interval(1);
        let (tx, rx) = mpsc::channel(8);
        state
            .autosync
            .subscribe(&state, ViewMode::GapsOnly, tx, None);
        assert_eq!(state.autosync.subscriber_count(), 1);
        // Drop the receiver: the next loop tick's fan-out prunes the sender.
        drop(rx);
        // Wait long enough for at least one tick (interval is 1 s).
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            state.autosync.subscriber_count(),
            0,
            "the dead subscriber was pruned"
        );
    }

    #[tokio::test]
    async fn zero_interval_never_spawns_the_loop() {
        let state = test_state_with_interval(0);
        let (tx, _rx) = mpsc::channel(8);
        state
            .autosync
            .subscribe(&state, ViewMode::GapsOnly, tx, None);
        // No loop task means the subscriber count stays put even after a
        // generous wait; pruning only happens inside the loop.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(state.autosync.subscriber_count(), 1);
    }

    #[tokio::test]
    async fn aborted_loop_respawns_on_next_subscribe() {
        let state = test_state_with_interval(60);
        let (tx1, _rx1) = mpsc::channel(8);
        state
            .autosync
            .subscribe(&state, ViewMode::GapsOnly, tx1, None);

        // Simulate a panic by aborting the loop task directly.
        state.autosync.abort_loop_for_test();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The next subscribe sees a finished JoinHandle (None after abort) and respawns.
        let (tx2, _rx2) = mpsc::channel(8);
        state
            .autosync
            .subscribe(&state, ViewMode::GapsOnly, tx2, None);
        let guard = state.autosync.inner.lock().unwrap();
        let handle = guard
            .loop_task
            .as_ref()
            .expect("respawn left a live JoinHandle");
        assert!(!handle.is_finished(), "respawned task is running");
    }
}
