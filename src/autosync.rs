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

/// Render one raw section as the OOB-swap string the autosync stream pushes.
/// Renders the raw section into a `RootSection` for the requested mode, then
/// delegates to `web::render::single_oob_section` so the OOB wrapping uses one
/// renderer shared with the page-level snapshot path (see ADR-0024).
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
        total_audiobooks: crate::service::count_audiobooks(&raw_section.state),
    };
    crate::web::render::single_oob_section(&rendered_section, root_idx, links, mode).into_string()
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

    /// Register a subscriber's mpsc sender under `mode` without seeding the
    /// per-mode baseline hashes. A test seam alongside `subscriber_count` and
    /// `abort_loop_for_test`: the loop's first tick after this call may emit
    /// a redundant section event for any root that has changed since the
    /// last broadcast, which is fine for tests that don't care about the
    /// first-tick diff. Production code wants `subscribe_and_seed`.
    ///
    /// Like `subscribe_and_seed`, this spawns the loop task if the registry
    /// was empty and the interval is non-zero, and the caller is responsible
    /// for sending the snapshot event into `sender` first so the channel's
    /// first event is always the snapshot.
    #[cfg(test)]
    pub(crate) fn subscribe(
        &self,
        state: &Arc<crate::state::AppState>,
        mode: ViewMode,
        sender: SseSender,
    ) {
        self.subscribe_inner(state, mode, sender, None);
    }

    /// Register a subscriber's mpsc sender under `mode` and seed the per-mode
    /// baseline hashes. Only the first subscriber for a mode writes the
    /// baseline: later subscribers receive truth via their own snapshot over
    /// their FIFO channel, and overwriting the shared baseline would erase
    /// pending diffs for any earlier tab. A new subscriber may receive one
    /// redundant section event on the next tick for a root that changed
    /// since the baseline was set, which is acceptable compared to the
    /// data-loss alternative.
    ///
    /// Spawns the loop task if the registry was empty and the interval is
    /// non-zero. The caller is responsible for sending the snapshot event
    /// into `sender` first so the channel's first event is always the
    /// snapshot.
    pub(crate) fn subscribe_and_seed(
        &self,
        state: &Arc<crate::state::AppState>,
        mode: ViewMode,
        sender: SseSender,
        seed_hashes: Vec<u64>,
    ) {
        self.subscribe_inner(state, mode, sender, Some(seed_hashes));
    }

    /// Shared body for `subscribe` and `subscribe_and_seed`. Keeps the
    /// register-then-maybe-seed-then-maybe-spawn ordering under one `guard`
    /// lock so the no-overwrite, lifecycle, and loop-spawn invariants land
    /// in one place. `Option<Vec<u64>>` is the internal carving; the two
    /// public methods name each caller intent at the surface.
    fn subscribe_inner(
        &self,
        state: &Arc<crate::state::AppState>,
        mode: ViewMode,
        sender: SseSender,
        seed_hashes: Option<Vec<u64>>,
    ) {
        let mut guard = self.inner.lock().expect("autosync mutex poisoned");
        guard.subs[mode].push(sender);
        if let Some(hashes) = seed_hashes
            && guard.last_hash[mode].is_empty()
        {
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

/// Atomically check whether the loop should exit (no subscribers in any mode)
/// and, if so, clear `loop_task` before returning. Holding the lock across
/// both the check and the clear means a subscriber arriving in the gap
/// cannot strand its registration against a loop that is about to exit.
fn try_exit_loop(inner: &StdMutex<AutosyncInner>) -> bool {
    let mut guard = inner.lock().expect("autosync mutex poisoned");
    if guard.subs.values().all(Vec::is_empty) {
        guard.loop_task = None;
        return true;
    }
    false
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
        // Exit cleanly if every subscriber has gone away. The check and the
        // loop_task clear happen under one lock acquisition so a subscriber
        // arriving in the gap cannot strand its registration against a dead
        // loop.
        if try_exit_loop(&inner) {
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
            .subscribe(&state, ViewMode::GapsOnly, tx);
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
            .subscribe(&state, ViewMode::GapsOnly, tx);
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
            .subscribe(&state, ViewMode::GapsOnly, tx1);

        // Simulate a panic by aborting the loop task directly.
        state.autosync.abort_loop_for_test();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The next subscribe sees a finished JoinHandle (None after abort) and respawns.
        let (tx2, _rx2) = mpsc::channel(8);
        state
            .autosync
            .subscribe(&state, ViewMode::GapsOnly, tx2);
        let guard = state.autosync.inner.lock().unwrap();
        let handle = guard
            .loop_task
            .as_ref()
            .expect("respawn left a live JoinHandle");
        assert!(!handle.is_finished(), "respawned task is running");
    }

    #[tokio::test]
    async fn second_subscribe_for_same_mode_does_not_overwrite_baseline_hashes() {
        // Interval of 60 s keeps the loop from ticking during the test so we can
        // observe the registry state without races against the loop body.
        let state = test_state_with_interval(60);

        let (tx1, _rx1) = mpsc::channel(8);
        let initial_hashes = vec![111u64, 222, 333];
        state.autosync.subscribe_and_seed(
            &state,
            ViewMode::GapsOnly,
            tx1,
            initial_hashes.clone(),
        );
        let before = state.autosync.inner.lock().unwrap().last_hash[ViewMode::GapsOnly].clone();
        assert_eq!(
            before,
            vec![Some(111), Some(222), Some(333)],
            "first subscribe seeds the baseline",
        );

        let (tx2, _rx2) = mpsc::channel(8);
        let later_hashes = vec![999u64, 999, 999];
        state
            .autosync
            .subscribe_and_seed(&state, ViewMode::GapsOnly, tx2, later_hashes);
        let after = state.autosync.inner.lock().unwrap().last_hash[ViewMode::GapsOnly].clone();
        assert_eq!(
            after, before,
            "second subscribe must not overwrite the baseline a prior subscriber set",
        );
    }

    #[tokio::test]
    async fn try_exit_loop_returns_false_and_keeps_loop_task_when_subs_present() {
        // A registry with one fake sender and an active-looking loop_task. The
        // helper must decline to exit and must not clear the task.
        let (tx, _rx) = mpsc::channel::<Result<Event, Infallible>>(1);
        let mut subs: EnumMap<ViewMode, Vec<SseSender>> = EnumMap::default();
        subs[ViewMode::GapsOnly].push(tx);
        let task = tokio::spawn(async {
            // Idle long enough that is_finished() stays false during the assert.
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let inner = Arc::new(StdMutex::new(AutosyncInner {
            subs,
            last_hash: EnumMap::default(),
            loop_task: Some(task),
        }));

        assert!(
            !try_exit_loop(&inner),
            "subscribers present means do not exit"
        );
        assert!(
            inner.lock().unwrap().loop_task.is_some(),
            "loop_task must survive a decline-to-exit",
        );
    }

    #[tokio::test]
    async fn try_exit_loop_returns_true_and_clears_loop_task_when_no_subs() {
        let task = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let inner = Arc::new(StdMutex::new(AutosyncInner {
            subs: EnumMap::default(),
            last_hash: EnumMap::default(),
            loop_task: Some(task),
        }));

        assert!(try_exit_loop(&inner), "no subscribers means exit");
        assert!(
            inner.lock().unwrap().loop_task.is_none(),
            "loop_task must be cleared atomically with the exit decision",
        );
    }

    #[test]
    fn render_oob_section_bytes_match_a_direct_single_oob_section_render() {
        // The contract from ADR-0024: the bytes a tab receives via SSE for a
        // root equal the bytes a Rescan click would render for the same root.
        // After consolidation both paths share single_oob_section; this test
        // pins that fact so a future divergence fails loudly.
        let raw = RawRootSection {
            path: "/some/root".to_string(),
            state: RawRootState::Clean,
        };
        let links: Vec<crate::config::SearchLink> = Vec::new();

        let via_autosync = render_oob_section(&raw, 7, ViewMode::GapsOnly, &links);

        let rendered_state =
            crate::service::render_root_state(&raw.path, &raw.state, ViewMode::GapsOnly);
        let rendered_section = crate::service::RootSection {
            path: raw.path.clone(),
            state: rendered_state,
            total_audiobooks: crate::service::count_audiobooks(&raw.state),
        };
        let via_render = crate::web::render::single_oob_section(
            &rendered_section,
            7,
            &links,
            ViewMode::GapsOnly,
        )
        .into_string();

        assert_eq!(via_autosync, via_render, "byte-equal SSE contract");
    }
}
