//! In-memory session table for the demo: which cookie maps to which set of marks,
//! and when each session was last seen. Bounded by a global cap; idle sessions
//! are reaped on a timer. Nothing here touches disk.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::marker::Marker;

/// An opaque session id carried in the visitor's cookie.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

/// One mark a visitor applied: which root, which folder (root-relative), and the
/// marker kind. Replayed on top of the base view to derive the visitor's view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// Index of the library root the mark targets.
    pub root: usize,
    /// Folder path relative to that root, or "." for the root itself.
    pub rel: String,
    /// Which marker the visitor chose.
    pub kind: Marker,
}

/// One visitor's private state: the marks they have applied, in submission order,
/// and when the session was last touched.
struct Session {
    marks: Vec<Mark>,
    last_seen: Instant,
}

/// Returned by `create` when the global session cap is reached.
#[derive(Debug, PartialEq, Eq)]
pub struct AtCapacity;

/// The session table and its global ceiling. One process holds one of these
/// behind a mutex; every operation runs under that single lock.
pub struct SessionStore {
    sessions: HashMap<SessionId, Session>,
    max_sessions: usize,
}

impl SessionStore {
    /// A new, empty store that admits up to `max_sessions` concurrent sessions.
    pub fn new(max_sessions: usize) -> SessionStore {
        SessionStore {
            sessions: HashMap::new(),
            max_sessions,
        }
    }

    /// How many sessions are live.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether no sessions are live.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Bump an existing session's last-seen to `now`. Returns whether the session
    /// existed; a `false` result means the cookie is unknown or was reaped.
    pub fn touch(&mut self, sid: &SessionId, now: Instant) -> bool {
        match self.sessions.get_mut(sid) {
            Some(session) => {
                session.last_seen = now;
                true
            }
            None => false,
        }
    }

    /// Create a fresh, empty session under the cap. Returns `Err(AtCapacity)` when
    /// the store is full, which the caller turns into the 503 page.
    pub fn create(&mut self, sid: SessionId, now: Instant) -> Result<(), AtCapacity> {
        if self.sessions.len() >= self.max_sessions {
            return Err(AtCapacity);
        }
        self.sessions.insert(
            sid,
            Session {
                marks: Vec::new(),
                last_seen: now,
            },
        );
        Ok(())
    }

    /// Append a mark to a session. A no-op when the session is gone.
    pub fn append_mark(&mut self, sid: &SessionId, mark: Mark) {
        if let Some(session) = self.sessions.get_mut(sid) {
            session.marks.push(mark);
        }
    }

    /// The marks a session has applied, in submission order. Empty when the
    /// session is unknown.
    pub fn marks(&self, sid: &SessionId) -> &[Mark] {
        self.sessions
            .get(sid)
            .map(|session| session.marks.as_slice())
            .unwrap_or(&[])
    }

    /// Drop every session idle for at least `idle` as of `now`; returns how many
    /// were dropped.
    pub fn reap_idle(&mut self, now: Instant, idle: Duration) -> usize {
        let before = self.sessions.len();
        self.sessions
            .retain(|_, session| now.duration_since(session.last_seen) < idle);
        before - self.sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use crate::marker::Marker;

    fn mark(root: usize, rel: &str) -> Mark {
        Mark {
            root,
            rel: rel.to_string(),
            kind: Marker::NoEbook,
        }
    }

    #[test]
    fn create_rejects_at_capacity() {
        let mut store = SessionStore::new(1);
        assert!(store.create(SessionId("a".into()), Instant::now()).is_ok());
        assert_eq!(
            store.create(SessionId("b".into()), Instant::now()),
            Err(AtCapacity)
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn touch_reports_presence_and_refreshes_last_seen() {
        let mut store = SessionStore::new(10);
        let old = Instant::now() - Duration::from_secs(3600);
        store.create(SessionId("a".into()), old).unwrap();
        // An unknown id is not present.
        assert!(!store.touch(&SessionId("missing".into()), Instant::now()));
        // Touching refreshes last_seen, so a reap that would have caught the
        // stale entry now spares it.
        let now = Instant::now();
        assert!(store.touch(&SessionId("a".into()), now));
        assert_eq!(store.reap_idle(now, Duration::from_secs(60)), 0);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn append_accumulates_marks_in_order() {
        let mut store = SessionStore::new(10);
        let sid = SessionId("a".into());
        store.create(sid.clone(), Instant::now()).unwrap();
        store.append_mark(&sid, mark(0, "Book"));
        store.append_mark(&sid, mark(1, "Author/Other"));
        assert_eq!(
            store.marks(&sid).to_vec(),
            vec![mark(0, "Book"), mark(1, "Author/Other")]
        );
    }

    #[test]
    fn marks_for_an_unknown_session_is_empty() {
        let store = SessionStore::new(10);
        assert!(store.marks(&SessionId("nope".into())).is_empty());
    }

    #[test]
    fn reap_drops_only_idle_sessions() {
        let mut store = SessionStore::new(10);
        let now = Instant::now();
        store.create(SessionId("fresh".into()), now).unwrap();
        store
            .create(SessionId("stale".into()), now - Duration::from_secs(3600))
            .unwrap();
        let reaped = store.reap_idle(now, Duration::from_secs(1200));
        assert_eq!(reaped, 1);
        assert_eq!(store.len(), 1);
        assert!(store.touch(&SessionId("fresh".into()), now));
        assert!(!store.touch(&SessionId("stale".into()), now));
    }
}
