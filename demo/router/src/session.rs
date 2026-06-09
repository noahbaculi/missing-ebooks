//! The in-memory session table: which cookie maps to which sandbox, when each
//! was last seen, and how many a given client IP holds. Spawning and killing
//! sandboxes happens elsewhere; this module only bookkeeps and enforces caps.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// An opaque session id carried in the visitor's cookie.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

/// One live sandbox: the loopback port it serves on, the child process id (so it
/// can be signalled), the client IP that owns it, and when it was last touched.
#[derive(Debug, Clone)]
pub struct Sandbox {
    pub port: u16,
    pub pid: u32,
    pub client_ip: String,
    pub last_seen: Instant,
}

/// Why a new sandbox cannot be created right now.
#[derive(Debug, PartialEq, Eq)]
pub enum AdmitError {
    /// The global ceiling on live sandboxes is reached.
    AtCapacity,
    /// This client IP already holds the maximum concurrent sandboxes.
    PerIpLimit,
}

/// The session table plus its two ceilings.
pub struct SessionStore {
    sandboxes: HashMap<SessionId, Sandbox>,
    /// In-flight spawns by IP, counted alongside committed sandboxes so that a
    /// burst of first-contact requests cannot all pass the cap check and then
    /// over-spawn while their launches are still running.
    reserved: HashMap<String, usize>,
    /// Total in-flight spawns, the global counterpart to `reserved`.
    reserved_total: usize,
    max_sandboxes: usize,
    max_per_ip: usize,
}

impl SessionStore {
    pub fn new(max_sandboxes: usize, max_per_ip: usize) -> Self {
        Self {
            sandboxes: HashMap::new(),
            reserved: HashMap::new(),
            reserved_total: 0,
            max_sandboxes,
            max_per_ip,
        }
    }

    pub fn live_count(&self) -> usize {
        self.sandboxes.len()
    }

    pub fn ip_count(&self, ip: &str) -> usize {
        self.sandboxes
            .values()
            .filter(|s| s.client_ip == ip)
            .count()
    }

    /// Look up the sandbox for a session, bumping its last-seen to `now` so the
    /// reaper treats an active visitor as fresh. Returns the serving port.
    pub fn touch(&mut self, sid: &SessionId, now: Instant) -> Option<u16> {
        let sandbox = self.sandboxes.get_mut(sid)?;
        sandbox.last_seen = now;
        Some(sandbox.port)
    }

    /// Reserve an in-flight slot for `ip` before its sandbox is spawned, or
    /// refuse it under the caps. Counting reservations alongside committed
    /// sandboxes is what closes the check-then-spawn race: the global cap first,
    /// then the per-IP cap. Every `Ok` must be paired with exactly one `commit`
    /// (spawn succeeded) or `release_reservation` (spawn failed).
    pub fn reserve(&mut self, ip: &str) -> Result<(), AdmitError> {
        if self.sandboxes.len() + self.reserved_total >= self.max_sandboxes {
            return Err(AdmitError::AtCapacity);
        }
        let ip_held = self.ip_count(ip) + self.reserved.get(ip).copied().unwrap_or(0);
        if ip_held >= self.max_per_ip {
            return Err(AdmitError::PerIpLimit);
        }
        self.reserved_total += 1;
        *self.reserved.entry(ip.to_string()).or_insert(0) += 1;
        Ok(())
    }

    /// Turn a reservation into a live sandbox: drop the in-flight slot and record
    /// the sandbox under its session id, both under one lock hold.
    pub fn commit(&mut self, ip: &str, sid: SessionId, sandbox: Sandbox) {
        self.release_reservation(ip);
        self.sandboxes.insert(sid, sandbox);
    }

    /// Hand back a reservation whose spawn never produced a sandbox.
    pub fn release_reservation(&mut self, ip: &str) {
        self.reserved_total = self.reserved_total.saturating_sub(1);
        if let Some(count) = self.reserved.get_mut(ip) {
            *count -= 1;
            if *count == 0 {
                self.reserved.remove(ip);
            }
        }
    }

    /// Record a freshly spawned sandbox under a session id, without touching the
    /// reservation counters. Tests use this to seed committed state directly.
    pub fn insert(&mut self, sid: SessionId, sandbox: Sandbox) {
        self.sandboxes.insert(sid, sandbox);
    }

    /// Remove and return every sandbox idle longer than `idle` as of `now`. The
    /// caller is responsible for killing the returned processes and releasing
    /// their ports.
    pub fn reap_idle(&mut self, now: Instant, idle: Duration) -> Vec<Sandbox> {
        let stale: Vec<SessionId> = self
            .sandboxes
            .iter()
            .filter(|(_, s)| now.duration_since(s.last_seen) >= idle)
            .map(|(sid, _)| sid.clone())
            .collect();
        stale
            .into_iter()
            .filter_map(|sid| self.sandboxes.remove(&sid))
            .collect()
    }

    /// Remove a single session, returning its sandbox if present. Used when a
    /// cookie points at a sandbox whose process has died.
    pub fn remove(&mut self, sid: &SessionId) -> Option<Sandbox> {
        self.sandboxes.remove(sid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(ip: &str, last_seen: Instant) -> Sandbox {
        Sandbox {
            port: 9000,
            pid: 1,
            client_ip: ip.to_string(),
            last_seen,
        }
    }

    #[test]
    fn reserve_rejects_at_global_capacity() {
        let mut store = SessionStore::new(1, 5);
        store.insert(SessionId("a".into()), sandbox("1.1.1.1", Instant::now()));
        assert_eq!(store.reserve("2.2.2.2"), Err(AdmitError::AtCapacity));
    }

    #[test]
    fn reserve_rejects_over_per_ip_limit() {
        let mut store = SessionStore::new(10, 1);
        store.insert(SessionId("a".into()), sandbox("1.1.1.1", Instant::now()));
        assert_eq!(store.reserve("1.1.1.1"), Err(AdmitError::PerIpLimit));
        assert!(store.reserve("2.2.2.2").is_ok());
    }

    #[test]
    fn reserve_counts_against_global_capacity() {
        let mut store = SessionStore::new(1, 5);
        assert!(store.reserve("1.1.1.1").is_ok());
        // The one global slot is reserved in-flight, so a second visitor is
        // refused even though no sandbox has been committed yet. This is the
        // race the plain admit check missed.
        assert_eq!(store.reserve("2.2.2.2"), Err(AdmitError::AtCapacity));
    }

    #[test]
    fn reserve_counts_against_per_ip_limit() {
        let mut store = SessionStore::new(10, 1);
        assert!(store.reserve("1.1.1.1").is_ok());
        // A concurrent second request from the same IP cannot slip past the
        // per-IP cap while the first spawn is still in flight.
        assert_eq!(store.reserve("1.1.1.1"), Err(AdmitError::PerIpLimit));
        assert!(store.reserve("2.2.2.2").is_ok());
    }

    #[test]
    fn committing_a_reservation_frees_the_inflight_slot() {
        let mut store = SessionStore::new(10, 2);
        assert!(store.reserve("1.1.1.1").is_ok());
        store.commit(
            "1.1.1.1",
            SessionId("a".into()),
            sandbox("1.1.1.1", Instant::now()),
        );
        // One committed sandbox plus room for one more, so a fresh reservation
        // still fits; the second one then fills the per-IP cap.
        assert!(store.reserve("1.1.1.1").is_ok());
        assert_eq!(store.reserve("1.1.1.1"), Err(AdmitError::PerIpLimit));
    }

    #[test]
    fn releasing_a_reservation_returns_the_slot() {
        let mut store = SessionStore::new(1, 5);
        assert!(store.reserve("1.1.1.1").is_ok());
        store.release_reservation("1.1.1.1");
        // A failed spawn hands its slot back, so the next visitor is admitted.
        assert!(store.reserve("2.2.2.2").is_ok());
    }

    #[test]
    fn touch_bumps_last_seen_and_returns_port() {
        let mut store = SessionStore::new(10, 10);
        let old = Instant::now() - Duration::from_secs(60);
        store.insert(SessionId("a".into()), sandbox("1.1.1.1", old));
        let now = Instant::now();
        assert_eq!(store.touch(&SessionId("a".into()), now), Some(9000));
        // A second reap window that would have caught the stale entry now spares
        // it, because touch refreshed last_seen.
        assert!(store.reap_idle(now, Duration::from_secs(30)).is_empty());
    }

    #[test]
    fn reap_returns_only_idle_sandboxes() {
        let mut store = SessionStore::new(10, 10);
        let now = Instant::now();
        store.insert(SessionId("fresh".into()), sandbox("1.1.1.1", now));
        store.insert(
            SessionId("stale".into()),
            sandbox("2.2.2.2", now - Duration::from_secs(3600)),
        );
        let reaped = store.reap_idle(now, Duration::from_secs(1200));
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].client_ip, "2.2.2.2");
        assert_eq!(store.live_count(), 1);
    }
}
