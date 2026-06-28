//! Background autosync. The `Autosync` struct on `AppState` owns a subscriber
//! registry and a single loop task; the loop wakes every
//! `autosync_interval_seconds` while at least one SSE client is connected,
//! diffs the rendered sections against the last broadcast, and pushes OOB
//! swap fragments for the ones that changed. See ADR-0023, ADR-0024.

use std::collections::hash_map::DefaultHasher;
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use axum::response::sse::Event;
use enum_map::EnumMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

use crate::state;
use crate::tree::ViewMode;

/// Diff each section's content hash against `last_content_hash` and return the
/// list of pushes, mutating `last_content_hash` in place. On a hash match the
/// loop skips the maud OOB-wrap render entirely; on a miss it wraps the
/// already-packaged section, updates the cache, and pushes. The hash is
/// per-mode (see `section_content_hash`) so mode-specific render-discarding
/// (e.g. show-all-only `cover_files` changes that the gaps view drops) does
/// not propagate as a push to the wrong mode.
///
/// `has_subs[mode]` short-circuits modes nobody is listening to: their hashes
/// stay untouched and they produce no pushes.
fn compute_pushes(
    raw: &state::RawView,
    last_content_hash: &mut EnumMap<ViewMode, Vec<Option<u64>>>,
    has_subs: EnumMap<ViewMode, bool>,
    links: &[crate::config::SearchLink],
) -> Vec<(ViewMode, usize, String)> {
    let mut pushes = Vec::new();
    for (root_idx, scan) in raw.iter().enumerate() {
        for mode in [ViewMode::GapsOnly, ViewMode::All] {
            if !has_subs[mode] {
                continue;
            }
            // Roots are config-fixed for a process, but the first call has
            // empty vecs.
            if last_content_hash[mode].len() != raw.len() {
                last_content_hash[mode].resize(raw.len(), None);
            }
            // Package once per (root, mode): the hash needs the packaged
            // section, and a cache miss reuses the same `section` to wrap
            // without paying for a second `package_section` call.
            let section = crate::web::render::package_section(scan, mode);
            let content_hash = section_content_hash(&section);
            if last_content_hash[mode][root_idx] == Some(content_hash) {
                continue;
            }
            let html = crate::web::render::single_oob_section(&section, root_idx, links, mode)
                .into_string();
            last_content_hash[mode][root_idx] = Some(content_hash);
            pushes.push((mode, root_idx, html));
        }
    }
    pushes
}

/// Render one raw section as the OOB-swap string the autosync stream pushes.
/// Renders the raw section into a `RootSection` for the requested mode through
/// `web::render::package_section` (one place owns the raw → packaged
/// step), then delegates to `web::render::single_oob_section` so the OOB
/// wrapping uses one renderer shared with the page-level snapshot path (see
/// ADR-0024).
///
/// Now only used by tests: `compute_pushes` and `snapshot_and_seed` inline the
/// two-step (package, then OOB-wrap) so a cache miss reuses the already
/// packaged section. The helper survives as the byte-equality contract the
/// `render_oob_section_bytes_match_a_direct_single_oob_section_render` test
/// pins.
#[cfg(test)]
fn render_oob_section(
    raw_section: &crate::scanner::RootScan,
    root_idx: usize,
    mode: ViewMode,
    links: &[crate::config::SearchLink],
) -> String {
    let rendered_section = crate::web::render::package_section(raw_section, mode);
    crate::web::render::single_oob_section(&rendered_section, root_idx, links, mode).into_string()
}

/// Hashes one packaged `RootSection` for the autosync dedup compare. The hash
/// is per-mode because the packaging discards inputs the mode does not render
/// (e.g. show-all-only `cover_files` on a covered audiobook collapse out of
/// the gaps `RootState`), so two modes can see different hashes for the same
/// `RootScan`. Shared by `snapshot_and_seed` (the seed hash a new subscriber
/// carries) and `compute_pushes` (the per-tick compare), so the seed and the
/// first-tick hash agree by construction (ADR-0024). Match implies equal
/// rendered HTML; the `content_hash_equals_render_parity` test pins that
/// contract, and `gaps_hash_unchanged_when_show_all_only_change_lands` pins
/// the per-mode isolation that lets the cache match for the gaps subscriber
/// when only show-all-relevant state shifts.
fn section_content_hash(section: &crate::web::render::RootSection) -> u64 {
    let mut hasher = DefaultHasher::new();
    section.hash(&mut hasher);
    hasher.finish()
}

/// Build the concatenated OOB-swap payload for an SSE `snapshot` event and the
/// per-root content hashes the autosync loop will use to suppress redundant
/// first-tick section events. The handler sends the payload, then passes the
/// hashes to `Autosync::subscribe` so the loop's first compute_pushes finds
/// matching hashes and emits nothing until something actually changes.
fn snapshot_and_seed(
    raw: &state::RawView,
    mode: ViewMode,
    links: &[crate::config::SearchLink],
) -> (String, Vec<u64>) {
    let mut payload = String::with_capacity(raw.len() * 512);
    let mut hashes = Vec::with_capacity(raw.len());
    for (root_idx, scan) in raw.iter().enumerate() {
        // Package once and reuse for both the rendered OOB fragment and the
        // seed hash, so the seed agrees byte-for-byte with what
        // `compute_pushes` will hash on the first tick.
        let section = crate::web::render::package_section(scan, mode);
        hashes.push(section_content_hash(&section));
        let oob =
            crate::web::render::single_oob_section(&section, root_idx, links, mode).into_string();
        payload.push_str(&oob);
    }
    (payload, hashes)
}

/// The per-root seed hashes a non-snapshot subscriber carries, computed
/// without rendering the OOB payload it would never send. Mirrors
/// `snapshot_and_seed`'s hashing so the seed agrees with `compute_pushes`.
fn seed_hashes(raw: &state::RawView, mode: ViewMode) -> Vec<u64> {
    raw.iter()
        .map(|scan| {
            let section = crate::web::render::package_section(scan, mode);
            section_content_hash(&section)
        })
        .collect()
}

/// Establish one SSE subscription and return the receiver the handler will
/// stream to the client. Owns the handshake (channel construction, ack send,
/// raw read, conditional snapshot send, registry subscription with seed
/// hashes) so the "ack before subscribe, snapshot before subscribe when sent"
/// ordering invariant lives in one place. See ADR-0023, ADR-0024, ADR-0030.
///
/// When `send_snapshot` is true, the channel sees `ack` then `snapshot`. When
/// false, only `ack`. In both cases the subscriber is registered with
/// `subscribe_and_seed` so the autosync loop's first tick suppresses
/// redundant section events for sections the inline render or the snapshot
/// already covered.
pub(crate) async fn attach(
    state: &Arc<crate::state::AppState>,
    mode: ViewMode,
    send_snapshot: bool,
) -> mpsc::Receiver<Result<Event, Infallible>> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(16);

    // Ack first on every connect: seeds the browser's lastEventId so a future
    // reconnect carries Last-Event-ID, regardless of whether a snapshot
    // follows now. See ADR-0030.
    let _ = tx.send(crate::web::ack_event()).await;

    // On a connect that sends the snapshot, render once and seed from the
    // same package call so the loop's first tick sees matching hashes
    // (ADR-0024). On a reconnect that carries Last-Event-ID, the snapshot
    // would be discarded, so skip the per-root OOB render and seed from the
    // hash alone.
    let raw = state.store.current().await;
    let seed_hashes = if send_snapshot {
        let (snapshot, hashes) = snapshot_and_seed(&raw, mode, &state.config.search_links);
        // Bumping the render count here keeps it consistent with the
        // snapshot_and_seed render the autosync loop's accounting expects.
        // Only the snapshot path bumps the counter, since that is the only
        // branch whose render reaches a subscriber.
        lock_inner(&state.autosync.inner)
            .render_count
            .fetch_add(hashes.len() as u64, Ordering::Relaxed);
        let _ = tx.send(crate::web::snapshot_event(snapshot)).await;
        hashes
    } else {
        seed_hashes(&raw, mode)
    };

    // Subscribe in both branches so the loop's first tick suppresses
    // redundant section events for sections the inline render or the
    // snapshot already covered. See ADR-0024.
    state
        .autosync
        .subscribe_and_seed(state, mode, tx, seed_hashes);

    rx
}

/// One subscriber's outbound channel. The loop fans out to every sender in
/// `subs[mode]`. A `try_send` failure prunes the sender.
pub(crate) type SseSender = mpsc::Sender<Result<Event, Infallible>>;

/// The shared autosync state for one process. Construct one per `AppState`.
/// The inner `Arc<Mutex<...>>` is cloned by the loop and by per-request handler
/// views, so the registry is one shared object regardless of how many `Autosync`
/// values point at it.
pub(crate) struct Autosync {
    inner: Arc<StdMutex<AutosyncInner>>,
    /// The configured idle gap. `0` disables the loop entirely. Subscribing
    /// still works (the snapshot is sent), but no loop task is ever spawned.
    interval: Duration,
}

/// The registry behind the lock: per-mode subscribers, per-(mode, root)
/// last-broadcast hashes, and the active loop task's handle.
struct AutosyncInner {
    subs: EnumMap<ViewMode, Vec<SseSender>>,
    last_content_hash: EnumMap<ViewMode, Vec<Option<u64>>>,
    /// Set while the loop is running; cleared by the loop on exit.
    loop_task: Option<JoinHandle<()>>,
    /// Monotonic count of every `single_oob_section` render observed by the
    /// autosync paths (snapshot seed and per-tick loop). Tests diff before
    /// vs. after to assert that no-change ticks skip the render. Mirrors
    /// `RawViewStore::rebuild_count` (`src/state.rs:54`). The field stays
    /// unconditional and the accessor is `#[cfg(test)]`; the runtime cost
    /// is one relaxed `fetch_add` per render path.
    render_count: AtomicU64,
}

impl Autosync {
    /// Build an empty registry. The loop is not spawned until the first
    /// subscriber arrives, even when `autosync_interval_seconds > 0`.
    #[must_use]
    pub(crate) fn new(autosync_interval_seconds: u64) -> Self {
        let inner = AutosyncInner {
            subs: EnumMap::default(),
            last_content_hash: EnumMap::default(),
            loop_task: None,
            render_count: AtomicU64::new(0),
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
    fn subscribe_and_seed(
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
        let mut guard = lock_inner(&self.inner);
        guard.subs[mode].push(sender);
        if let Some(hashes) = seed_hashes
            && guard.last_content_hash[mode].is_empty()
        {
            guard.last_content_hash[mode] = hashes.into_iter().map(Some).collect();
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

    /// Number of active subscribers across both modes. Tests reach in.
    /// Production code does not.
    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        let guard = lock_inner(&self.inner);
        guard.subs.values().map(Vec::len).sum()
    }

    /// Whether `last_content_hash[mode]` carries any entries. Tests use this
    /// to assert that `attach` seeded the baseline in both branches, since
    /// `subscribe_and_seed` runs whether or not a snapshot was sent.
    #[cfg(test)]
    pub(crate) fn has_seeded_baseline_for_test(&self, mode: ViewMode) -> bool {
        let guard = lock_inner(&self.inner);
        !guard.last_content_hash[mode].is_empty()
    }

    /// Monotonic render count: every `single_oob_section` produced by either
    /// the snapshot seed path or the per-tick loop. Tests diff before vs.
    /// after to assert that no-change ticks skip the render.
    #[cfg(test)]
    pub(crate) fn render_count(&self) -> u64 {
        lock_inner(&self.inner).render_count.load(Ordering::Relaxed)
    }

    /// Abort the loop task without removing subscribers. Tests use this to
    /// simulate a panic inside the loop and confirm the next subscribe
    /// respawns.
    #[cfg(test)]
    pub(crate) fn abort_loop_for_test(&self) {
        let mut guard = lock_inner(&self.inner);
        if let Some(h) = guard.loop_task.take() {
            h.abort();
        }
    }
}

/// Lock the autosync inner mutex, recovering the guard when a previous
/// holder panicked. The registry is insert/remove only and remains valid
/// after a poisoned guard, so recovery beats wedging the SSE loop on a
/// transient render panic. The poison-recovery pattern duplicates `DirIndex`'s
/// internal lock recovery; this variant logs a warn because a poisoned autosync
/// registry typically indicates a render or sender bug worth surfacing.
fn lock_inner(inner: &StdMutex<AutosyncInner>) -> std::sync::MutexGuard<'_, AutosyncInner> {
    inner.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("autosync inner mutex poisoned; recovering");
        poisoned.into_inner()
    })
}

/// Atomically check whether the loop should exit (no subscribers in any mode)
/// and, if so, clear `loop_task` before returning. Holding the lock across
/// both the check and the clear means a subscriber arriving in the gap
/// cannot strand its registration against a loop that is about to exit.
fn try_exit_loop(inner: &StdMutex<AutosyncInner>) -> bool {
    let mut guard = lock_inner(inner);
    if guard.subs.values().all(Vec::is_empty) {
        guard.loop_task = None;
        return true;
    }
    false
}

/// The loop body. Holds a `Weak<AppState>` so the application can drop without
/// leaking the loop. A failed upgrade per tick means the process is shutting
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

        // Single-flighted with /rescan and page-load rebuilds via RawViewStore::refresh.
        let raw = state.store.refresh().await;

        // Render and diff under the registry lock. The critical section is
        // short: per-section render is microseconds (ADR-0022) and there is no
        // await between lock and unlock. The render-count bump folds into the
        // same guard so it stays atomic with the slot writes that produced it.
        let to_send: Vec<(ViewMode, usize, String)> = {
            let mut guard = lock_inner(&inner);
            let has_subs = EnumMap::from_fn(|mode| !guard.subs[mode].is_empty());
            let pushes = compute_pushes(
                &raw,
                &mut guard.last_content_hash,
                has_subs,
                &state.config.search_links,
            );
            // `compute_pushes` calls `single_oob_section` exactly once per
            // returned push, so `pushes.len()` is the exact render count for
            // this tick.
            guard
                .render_count
                .fetch_add(pushes.len() as u64, Ordering::Relaxed);
            pushes
        };

        // Fan out and prune. A failed try_send drops that sender from the list.
        if !to_send.is_empty() {
            let mut guard = lock_inner(&inner);
            for (mode, _root_idx, html) in to_send {
                let event = crate::web::section_event(html);
                guard.subs[mode].retain(|tx| tx.try_send(Ok(event.clone())).is_ok());
            }
        } else {
            // Even with no pushes, prune any senders whose receiver already
            // dropped, so the loop notices a quiet client disappearing and
            // can exit on the next iteration when the last sub goes away.
            let mut guard = lock_inner(&inner);
            for mode in [ViewMode::GapsOnly, ViewMode::All] {
                guard.subs[mode].retain(|tx| !tx.is_closed());
            }
        }

        sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::RootScan;
    use enum_map::enum_map;

    fn both_modes_subscribed() -> EnumMap<ViewMode, bool> {
        enum_map! { ViewMode::GapsOnly => true, ViewMode::All => true }
    }

    fn empty_hashes() -> EnumMap<ViewMode, Vec<Option<u64>>> {
        enum_map! { ViewMode::GapsOnly => Vec::new(), ViewMode::All => Vec::new() }
    }

    fn walked_root_with_folder(i: usize, missing_ebook: bool) -> RootScan {
        use crate::scanner::ScannedFolder;
        use std::path::PathBuf;
        RootScan::Walked {
            canonical_path: PathBuf::from(format!("/root/{i}")),
            folders: vec![ScannedFolder {
                rel_path: PathBuf::from("Book"),
                directly_holds_audio: true,
                missing_ebook,
                cover_files: std::sync::Arc::from(Vec::<String>::new()),
                audio_files: std::sync::Arc::from(vec!["01.mp3".to_string()]),
            }],
        }
    }

    fn raw_view_of(roots: Vec<RootScan>) -> state::RawView {
        roots
    }

    fn no_links() -> Vec<crate::config::SearchLink> {
        Vec::new()
    }

    #[test]
    fn lock_inner_recovers_from_poisoning() {
        // Build a minimal inner registry directly.
        let inner = Arc::new(StdMutex::new(AutosyncInner {
            subs: EnumMap::default(),
            last_content_hash: EnumMap::default(),
            loop_task: None,
            render_count: AtomicU64::new(0),
        }));

        // Poison the mutex: take the guard on a worker thread, then panic.
        let poisoner_inner = Arc::clone(&inner);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner_inner.lock().unwrap();
            panic!("intentional poison for test");
        })
        .join();

        // The mutex is now poisoned. The bare std API would panic on .expect(...).
        assert!(
            inner.lock().is_err(),
            "test setup failed: mutex was not poisoned"
        );

        // lock_inner must recover and return a usable guard.
        let guard = lock_inner(&inner);
        assert!(
            guard.loop_task.is_none(),
            "recovered guard exposes the prior state"
        );
        drop(guard);
    }

    #[test]
    fn first_call_pushes_every_mode_root_pair() {
        let raw = raw_view_of(vec![
            walked_root_with_folder(0, true),
            walked_root_with_folder(1, true),
        ]);
        let mut hashes = empty_hashes();
        let links = no_links();
        let pushes = compute_pushes(&raw, &mut hashes, both_modes_subscribed(), &links);
        assert_eq!(pushes.len(), 4, "two modes times two roots");
        assert!(pushes.iter().all(|(_, _, html)| !html.is_empty()));
    }

    #[test]
    fn identical_second_call_pushes_nothing() {
        let raw = raw_view_of(vec![
            walked_root_with_folder(0, true),
            walked_root_with_folder(1, true),
        ]);
        let mut hashes = empty_hashes();
        let links = no_links();
        let _first = compute_pushes(&raw, &mut hashes, both_modes_subscribed(), &links);
        let second = compute_pushes(&raw, &mut hashes, both_modes_subscribed(), &links);
        assert!(second.is_empty(), "no roots changed, no pushes");
    }

    #[test]
    fn changed_root_produces_pushes_only_for_that_root() {
        // Seed: render three roots, all gap-bearing.
        let raw_before = raw_view_of(vec![
            walked_root_with_folder(0, true),
            walked_root_with_folder(1, true),
            walked_root_with_folder(2, true),
        ]);
        let mut hashes = empty_hashes();
        let links = no_links();
        let _ = compute_pushes(&raw_before, &mut hashes, both_modes_subscribed(), &links);
        let hashes_before = hashes.clone();

        // Mutate root 1's folder so its rendered HTML changes for every mode.
        let raw_after = raw_view_of(vec![
            walked_root_with_folder(0, true),
            walked_root_with_folder(1, false), // missing_ebook flipped
            walked_root_with_folder(2, true),
        ]);
        let pushes = compute_pushes(&raw_after, &mut hashes, both_modes_subscribed(), &links);

        // Both subscribed modes push for root 1; no other root pushes.
        assert!(
            pushes.iter().all(|(_, root_idx, _)| *root_idx == 1),
            "only root 1 pushed: {pushes:?}",
        );
        assert!(!pushes.is_empty(), "at least one mode saw a real diff");

        // Roots 0 and 2 untouched in both modes.
        for mode in [ViewMode::GapsOnly, ViewMode::All] {
            assert_eq!(
                hashes[mode][0], hashes_before[mode][0],
                "root 0 unchanged in {mode:?}"
            );
            assert_eq!(
                hashes[mode][2], hashes_before[mode][2],
                "root 2 unchanged in {mode:?}"
            );
        }
    }

    #[test]
    fn mode_with_no_subscribers_is_skipped_and_its_hashes_stay_untouched() {
        let raw = raw_view_of(vec![
            walked_root_with_folder(0, true),
            walked_root_with_folder(1, true),
        ]);
        let mut hashes = empty_hashes();
        let links = no_links();
        let only_gaps = enum_map! { ViewMode::GapsOnly => true, ViewMode::All => false };
        let pushes = compute_pushes(&raw, &mut hashes, only_gaps, &links);
        assert_eq!(pushes.len(), 2, "two roots in GapsOnly only");
        assert!(pushes.iter().all(|(m, _, _)| *m == ViewMode::GapsOnly));
        assert!(
            hashes[ViewMode::All].is_empty(),
            "the All mode's hashes were never resized or written",
        );
    }

    use std::path::PathBuf;

    fn test_state_with_interval(autosync_interval_seconds: u64) -> Arc<crate::state::AppState> {
        let dir = tempfile::tempdir().expect("tempdir");
        let scenario = crate::scenarios::find_scenario("clean-error").expect("scenario exists");
        let roots: Vec<PathBuf> = crate::scenarios::materialize(&(scenario.spec)(), dir.path());
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
        state.autosync.subscribe(&state, ViewMode::GapsOnly, tx);
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
        state.autosync.subscribe(&state, ViewMode::GapsOnly, tx);
        // No loop task means the subscriber count stays put even after a
        // generous wait; pruning only happens inside the loop.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(state.autosync.subscriber_count(), 1);
    }

    #[tokio::test]
    async fn aborted_loop_respawns_on_next_subscribe() {
        let state = test_state_with_interval(60);
        let (tx1, _rx1) = mpsc::channel(8);
        state.autosync.subscribe(&state, ViewMode::GapsOnly, tx1);

        // Simulate a panic by aborting the loop task directly.
        state.autosync.abort_loop_for_test();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The next subscribe sees a finished JoinHandle (None after abort) and respawns.
        let (tx2, _rx2) = mpsc::channel(8);
        state.autosync.subscribe(&state, ViewMode::GapsOnly, tx2);
        let guard = state.autosync.inner.lock().unwrap();
        let handle = guard
            .loop_task
            .as_ref()
            .expect("respawn left a live JoinHandle");
        assert!(!handle.is_finished(), "respawned task is running");
    }

    #[tokio::test]
    async fn no_change_tick_does_not_render() {
        // Attach both modes, capture the post-snapshot render floor, sleep
        // long enough for several loop ticks, and assert the counter did not
        // grow. `attach` renders one section per root for the snapshot, then
        // seeds the per-mode baseline hashes; with a stable filesystem the
        // loop's subsequent ticks must find matching hashes and skip both
        // the OOB-wrap render and the push.
        let state = test_state_with_interval(1);

        let _rx_gaps = attach(&state, ViewMode::GapsOnly, true).await;
        let _rx_all = attach(&state, ViewMode::All, true).await;

        let snapshot_floor = state.autosync.render_count();
        assert!(
            snapshot_floor > 0,
            "snapshot path must have rendered at least one section",
        );

        // 2.5 s on a 1 s interval is long enough for at least two ticks
        // without making the test painfully slow.
        tokio::time::sleep(Duration::from_millis(2500)).await;

        assert_eq!(
            state.autosync.render_count(),
            snapshot_floor,
            "no-change ticks must not render",
        );
    }

    #[tokio::test]
    async fn attach_with_send_snapshot_true_emits_ack_then_snapshot_and_registers_subscriber() {
        // Interval high enough that the autosync loop will not tick during the
        // test, so we observe the post-attach state without races.
        let state = test_state_with_interval(60);

        let mut rx = attach(&state, ViewMode::GapsOnly, true).await;

        // Two events land on the channel: ack first, snapshot second. Axum's
        // `Event` does not expose its name or data via getters, so we match a
        // substring of its Debug output. TODO(axum): switch to a structural
        // check (or an on-the-wire SSE-frame check) when axum exposes
        // accessors; the Debug format is not part of axum's public contract.
        let first = tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("attach must send the ack before returning")
            .expect("the channel must yield an Ok(Event)")
            .expect("the inner Result must be Ok");
        assert!(
            format!("{first:?}").contains("event: ack"),
            "first event must be the ack, got: {first:?}"
        );

        let second = tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("snapshot follows the ack when send_snapshot is true")
            .expect("Ok")
            .expect("Ok");
        assert!(
            format!("{second:?}").contains("event: snapshot"),
            "second event must be the snapshot, got: {second:?}"
        );

        // The subscriber landed in the registry under GapsOnly.
        assert_eq!(
            state.autosync.subscriber_count(),
            1,
            "attach registers exactly one subscriber",
        );

        // The per-mode baseline was seeded so the loop's first tick would
        // suppress redundant section events for unchanged roots.
        let baseline =
            state.autosync.inner.lock().unwrap().last_content_hash[ViewMode::GapsOnly].clone();
        assert!(
            !baseline.is_empty(),
            "attach seeded the GapsOnly baseline hashes",
        );
        assert!(
            baseline.iter().all(Option::is_some),
            "every root got a seeded hash, not None",
        );
    }

    #[tokio::test]
    async fn attach_without_snapshot_emits_only_ack_and_registers_subscriber() {
        let state = test_state_with_interval(0);
        let mut rx = attach(&state, ViewMode::GapsOnly, false).await;

        // First event is the ack.
        let event = tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("attach must send the ack before returning")
            .expect("the channel must yield an Ok(Event)")
            .expect("the inner Result must be Ok");
        let serialized = format!("{event:?}");
        assert!(
            serialized.contains("event: ack"),
            "first event must be ack, got {serialized}",
        );

        // No second event arrives within a brief window: no snapshot was sent.
        let next = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(
            next.is_err(),
            "no snapshot should follow the ack on first connect",
        );

        assert_eq!(
            state.autosync.subscriber_count(),
            1,
            "attach registers exactly one subscriber regardless of snapshot",
        );
        assert_eq!(
            state.autosync.render_count(),
            0,
            "no snapshot path means no render bumps",
        );
    }

    #[tokio::test]
    async fn attach_seeds_baseline_hashes_in_both_branches() {
        // send_snapshot: false still seeds last_content_hash so the loop's
        // first tick does not redundantly broadcast the inline-rendered state.
        let state = test_state_with_interval(0);
        let _rx = attach(&state, ViewMode::GapsOnly, false).await;
        assert!(
            state
                .autosync
                .has_seeded_baseline_for_test(ViewMode::GapsOnly),
            "attach must seed last_content_hash even when it skipped the snapshot",
        );
    }

    #[test]
    fn section_event_helper_stamps_id_for_reconnect() {
        // The per-tick path now routes through crate::web::section_event so
        // every section event carries the same id: r stamp the snapshot and
        // ack carry. A drop after the loop has pushed at least one section
        // therefore reconnects with Last-Event-ID, which the /events
        // handler treats as a reconnect.
        let event = crate::web::section_event("<oob>x</oob>".to_string());
        assert!(
            format!("{event:?}").contains("id: r"),
            "section_event must carry id: r so a reconnect after a section push sends Last-Event-ID",
        );
    }

    #[tokio::test]
    async fn second_subscribe_for_same_mode_does_not_overwrite_baseline_hashes() {
        // Interval of 60 s keeps the loop from ticking during the test so we can
        // observe the registry state without races against the loop body.
        let state = test_state_with_interval(60);

        let (tx1, _rx1) = mpsc::channel(8);
        let initial_hashes = vec![111u64, 222, 333];
        state
            .autosync
            .subscribe_and_seed(&state, ViewMode::GapsOnly, tx1, initial_hashes.clone());
        let before =
            state.autosync.inner.lock().unwrap().last_content_hash[ViewMode::GapsOnly].clone();
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
        let after =
            state.autosync.inner.lock().unwrap().last_content_hash[ViewMode::GapsOnly].clone();
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
            last_content_hash: EnumMap::default(),
            loop_task: Some(task),
            render_count: AtomicU64::new(0),
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
            last_content_hash: EnumMap::default(),
            loop_task: Some(task),
            render_count: AtomicU64::new(0),
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
        // pins that fact so a future divergence fails loudly. Derive the
        // rendered section through web::render::package_section — the
        // helper render_oob_section itself uses — so a drift in that helper
        // fails this test rather than getting silently re-applied here.
        let raw = RootScan::Walked {
            canonical_path: std::path::PathBuf::from("/some/root"),
            folders: Vec::new(),
        };
        let links: Vec<crate::config::SearchLink> = Vec::new();

        let via_autosync = render_oob_section(&raw, 7, ViewMode::GapsOnly, &links);

        let rendered_section = crate::web::render::package_section(&raw, ViewMode::GapsOnly);
        let via_render = crate::web::render::single_oob_section(
            &rendered_section,
            7,
            &links,
            ViewMode::GapsOnly,
        )
        .into_string();

        assert_eq!(via_autosync, via_render, "byte-equal SSE contract");
    }

    #[test]
    fn render_oob_section_html_carries_total_audiobooks_for_a_walked_root() {
        use crate::scanner::ScannedFolder;
        use std::path::PathBuf;

        let raw = RootScan::Walked {
            canonical_path: PathBuf::from("/lib"),
            folders: vec![
                ScannedFolder {
                    rel_path: PathBuf::from("Book"),
                    directly_holds_audio: true,
                    missing_ebook: true,
                    cover_files: std::sync::Arc::from(Vec::<String>::new()),
                    audio_files: std::sync::Arc::from(vec!["01.mp3".to_string()]),
                },
                ScannedFolder {
                    rel_path: PathBuf::from("Container"),
                    directly_holds_audio: false,
                    missing_ebook: false,
                    cover_files: std::sync::Arc::from(Vec::<String>::new()),
                    audio_files: std::sync::Arc::from(Vec::<String>::new()),
                },
            ],
        };
        let html = render_oob_section(&raw, 0, ViewMode::GapsOnly, &[]);
        // The pushed fragment includes the data attr on its section open tag,
        // so the live page's coverage stays current after an autosync swap.
        assert!(html.contains(r#"data-total-audiobooks="1""#));
    }

    #[test]
    fn content_hash_equals_render_parity() {
        // Equality of section_content_hash must imply equality of rendered HTML
        // for the same mode, so compute_pushes can skip the OOB wrap on a hash
        // match without dropping a real diff. Fails closed if a future
        // renderer input lands outside the packaged section.
        let a = walked_root_with_folder(0, true);
        let b = walked_root_with_folder(0, true);
        let links = no_links();

        let section_a = crate::web::render::package_section(&a, ViewMode::GapsOnly);
        let section_b = crate::web::render::package_section(&b, ViewMode::GapsOnly);
        assert_eq!(
            section_content_hash(&section_a),
            section_content_hash(&section_b),
        );
        assert_eq!(
            render_oob_section(&a, 0, ViewMode::GapsOnly, &links),
            render_oob_section(&b, 0, ViewMode::GapsOnly, &links),
        );

        // Flip one bit of content
        let c = walked_root_with_folder(0, false);
        let section_c = crate::web::render::package_section(&c, ViewMode::GapsOnly);
        assert_ne!(
            section_content_hash(&section_a),
            section_content_hash(&section_c),
        );
        assert_ne!(
            render_oob_section(&a, 0, ViewMode::GapsOnly, &links),
            render_oob_section(&c, 0, ViewMode::GapsOnly, &links),
        );
    }

    #[test]
    fn gaps_hash_unchanged_when_show_all_only_change_lands() {
        // Per-mode dedup property: a change that only the show-all renderer
        // sees (here, adding a second cover file on a covered audiobook) must
        // leave the gaps-mode content hash equal, so the gaps subscriber
        // receives no push. tests/sse.rs::two_modes_isolated is the end-to-end
        // version of this contract; this test pins the underlying invariant
        // at the hash level.
        use crate::scanner::{RootScan, ScannedFolder};
        use std::path::PathBuf;

        let covered = |cover_files: Vec<String>| RootScan::Walked {
            canonical_path: PathBuf::from("/lib"),
            folders: vec![ScannedFolder {
                rel_path: PathBuf::from("Book"),
                directly_holds_audio: true,
                missing_ebook: false,
                cover_files: cover_files.into(),
                audio_files: std::sync::Arc::from(vec!["01.mp3".to_string()]),
            }],
        };
        let before = covered(vec!["Book.epub".to_string()]);
        let after = covered(vec![
            "Book.epub".to_string(),
            "Book.companion.epub".to_string(),
        ]);

        let gaps_before = section_content_hash(&crate::web::render::package_section(
            &before,
            ViewMode::GapsOnly,
        ));
        let gaps_after = section_content_hash(&crate::web::render::package_section(
            &after,
            ViewMode::GapsOnly,
        ));
        assert_eq!(
            gaps_before, gaps_after,
            "gaps mode discards cover_files; hash must be stable across this change",
        );

        // The same change does flip the show-all hash, otherwise show-all
        // would never see the push it must see.
        let all_before =
            section_content_hash(&crate::web::render::package_section(&before, ViewMode::All));
        let all_after =
            section_content_hash(&crate::web::render::package_section(&after, ViewMode::All));
        assert_ne!(
            all_before, all_after,
            "show-all carries cover_files; hash must change so the push fires",
        );
    }
}
