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
    max_sandboxes: usize,
    max_per_ip: usize,
}

impl SessionStore {
    pub fn new(max_sandboxes: usize, max_per_ip: usize) -> Self {
        Self {
            sandboxes: HashMap::new(),
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

    /// Decide whether `ip` may create another sandbox. Checks the global cap
    /// first, then the per-IP cap.
    pub fn admit(&self, ip: &str) -> Result<(), AdmitError> {
        if self.live_count() >= self.max_sandboxes {
            return Err(AdmitError::AtCapacity);
        }
        if self.ip_count(ip) >= self.max_per_ip {
            return Err(AdmitError::PerIpLimit);
        }
        Ok(())
    }

    /// Record a freshly spawned sandbox under a session id.
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
    fn admit_rejects_at_global_capacity() {
        let mut store = SessionStore::new(1, 5);
        store.insert(SessionId("a".into()), sandbox("1.1.1.1", Instant::now()));
        assert_eq!(store.admit("2.2.2.2"), Err(AdmitError::AtCapacity));
    }

    #[test]
    fn admit_rejects_over_per_ip_limit() {
        let mut store = SessionStore::new(10, 1);
        store.insert(SessionId("a".into()), sandbox("1.1.1.1", Instant::now()));
        assert_eq!(store.admit("1.1.1.1"), Err(AdmitError::PerIpLimit));
        assert!(store.admit("2.2.2.2").is_ok());
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
        store.insert(
            SessionId("fresh".into()),
            sandbox("1.1.1.1", now),
        );
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
