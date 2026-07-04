//! In-memory session table for the demo: which cookie maps to which set of marks,
//! and when each session was last seen. Bounded by a global cap. Idle sessions
//! are reaped on a timer. Nothing here touches disk.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::marker::Marker;

/// An opaque session id carried in the visitor's cookie.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SessionId(String);

impl SessionId {
    pub(crate) fn new(value: String) -> SessionId {
        SessionId(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// One mark in the session's set: the library root index, the folder path
/// relative to that root (or "." for the root itself, per ADR-0005), and the
/// marker kind. The set is keyed on this tuple, so repeated identical marks
/// are no-ops at insert time and per-session size is structurally bounded by
/// the scenario's `|markable folders x marker kinds|`.
pub(crate) type MarkKey = (usize, String, Marker);

/// One visitor's private state: the marks they have applied as a set, and when
/// the session was last touched.
struct Session {
    marks: HashSet<MarkKey>,
    last_seen: Instant,
}

/// Returned by `create` when the global session cap is reached.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AtCapacity;

/// The session table and its global ceiling. One process holds one of these
/// behind a mutex. Every operation runs under that single lock.
pub(crate) struct SessionStore {
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
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Bump an existing session's last-seen to `now`. Returns whether the session
    /// existed. A `false` result means the cookie is unknown or was reaped.
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
                marks: HashSet::new(),
                last_seen: now,
            },
        );
        Ok(())
    }

    /// Insert a mark into a session. Returns `true` when newly added,
    /// `false` when the mark was already present or the session is gone.
    /// Silent no-op on unknown sessions matches today's `append_mark` shape.
    /// Handlers always call this immediately after `resolve_in_store`, so
    /// the session-gone branch is a logic-bug guard, not a user-reachable path.
    pub fn insert_mark(&mut self, sid: &SessionId, key: MarkKey) -> bool {
        match self.sessions.get_mut(sid) {
            Some(session) => session.marks.insert(key),
            None => false,
        }
    }

    /// Remove a mark from a session. Returns `true` when the mark was
    /// present and removed, `false` when absent or the session is gone.
    pub fn remove_mark(&mut self, sid: &SessionId, key: &MarkKey) -> bool {
        match self.sessions.get_mut(sid) {
            Some(session) => session.marks.remove(key),
            None => false,
        }
    }

    /// Empty a session's marks, leaving the session in place. A no-op when the
    /// session is gone.
    pub fn clear_marks(&mut self, sid: &SessionId) {
        if let Some(session) = self.sessions.get_mut(sid) {
            session.marks.clear();
        }
    }

    /// The marks a session has applied as a set. Empty when the session is
    /// unknown. Borrowed for the duration of the caller's lock guard. The
    /// render path consumes this reference directly without copying.
    pub fn marks(&self, sid: &SessionId) -> &HashSet<MarkKey> {
        // A static empty set so the unknown-session path can return a
        // `&HashSet` without a per-call allocation. `OnceLock` keeps it
        // const-eval-free without an unsafe `static mut` or a per-call
        // `Box::leak`.
        static EMPTY: std::sync::OnceLock<HashSet<MarkKey>> = std::sync::OnceLock::new();
        self.sessions
            .get(sid)
            .map(|session| &session.marks)
            .unwrap_or_else(|| EMPTY.get_or_init(HashSet::new))
    }

    /// Drop every session idle for at least `idle` as of `now`. Returns how many
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

    fn key(root: usize, rel: &str) -> MarkKey {
        (root, rel.to_string(), Marker::NoEbook)
    }

    #[test]
    fn create_rejects_at_capacity() {
        let mut store = SessionStore::new(1);
        assert!(
            store
                .create(SessionId::new("a".into()), Instant::now())
                .is_ok()
        );
        assert_eq!(
            store.create(SessionId::new("b".into()), Instant::now()),
            Err(AtCapacity)
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn touch_reports_presence_and_refreshes_last_seen() {
        let mut store = SessionStore::new(10);
        let old = Instant::now() - Duration::from_secs(3600);
        store.create(SessionId::new("a".into()), old).unwrap();
        assert!(!store.touch(&SessionId::new("missing".into()), Instant::now()));
        let now = Instant::now();
        assert!(store.touch(&SessionId::new("a".into()), now));
        assert_eq!(store.reap_idle(now, Duration::from_secs(60)), 0);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn marks_for_an_unknown_session_is_empty() {
        let store = SessionStore::new(10);
        assert!(store.marks(&SessionId::new("nope".into())).is_empty());
    }

    #[test]
    fn reap_drops_only_idle_sessions() {
        let mut store = SessionStore::new(10);
        let now = Instant::now();
        store.create(SessionId::new("fresh".into()), now).unwrap();
        store
            .create(
                SessionId::new("stale".into()),
                now - Duration::from_secs(3600),
            )
            .unwrap();
        let reaped = store.reap_idle(now, Duration::from_secs(1200));
        assert_eq!(reaped, 1);
        assert_eq!(store.len(), 1);
        assert!(store.touch(&SessionId::new("fresh".into()), now));
        assert!(!store.touch(&SessionId::new("stale".into()), now));
    }

    #[test]
    fn clear_marks_empties_a_session_and_leaves_others() {
        let mut store = SessionStore::new(10);
        let a = SessionId::new("a".into());
        let b = SessionId::new("b".into());
        store.create(a.clone(), Instant::now()).unwrap();
        store.create(b.clone(), Instant::now()).unwrap();
        store.insert_mark(&a, key(0, "Book"));
        store.insert_mark(&b, key(1, "Other"));

        store.clear_marks(&a);
        assert!(store.marks(&a).is_empty());
        assert_eq!(store.marks(&b).len(), 1);
        assert!(store.marks(&b).contains(&key(1, "Other")));

        // Clearing an unknown id is a no-op and does not create a session.
        store.clear_marks(&SessionId::new("missing".into()));
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn insert_mark_dedupes() {
        let mut store = SessionStore::new(8);
        let sid = SessionId::new("s1".to_string());
        let now = Instant::now();
        store.create(sid.clone(), now).unwrap();

        let k = (0_usize, "Author/Book".to_string(), Marker::NoEbook);
        assert!(store.insert_mark(&sid, k.clone()), "first insert is new");
        assert!(
            !store.insert_mark(&sid, k.clone()),
            "second insert is a dup"
        );
        assert_eq!(store.marks(&sid).len(), 1);
    }

    #[test]
    fn marks_set_is_per_session() {
        let mut store = SessionStore::new(8);
        let s1 = SessionId::new("s1".to_string());
        let s2 = SessionId::new("s2".to_string());
        let now = Instant::now();
        store.create(s1.clone(), now).unwrap();
        store.create(s2.clone(), now).unwrap();

        store.insert_mark(&s1, (0, "A".to_string(), Marker::NoEbook));
        assert_eq!(store.marks(&s1).len(), 1);
        assert_eq!(store.marks(&s2).len(), 0);
    }

    #[test]
    fn clear_marks_empties_the_set() {
        let mut store = SessionStore::new(8);
        let sid = SessionId::new("s1".to_string());
        let now = Instant::now();
        store.create(sid.clone(), now).unwrap();
        store.insert_mark(&sid, (0, "A".to_string(), Marker::NoEbook));
        store.insert_mark(&sid, (0, "B".to_string(), Marker::EbookElsewhere));
        assert_eq!(store.marks(&sid).len(), 2);

        store.clear_marks(&sid);
        assert_eq!(store.marks(&sid).len(), 0);
    }

    #[test]
    fn remove_mark_returns_whether_present() {
        let mut store = SessionStore::new(8);
        let sid = SessionId::new("s1".to_string());
        let now = Instant::now();
        store.create(sid.clone(), now).unwrap();
        let k = (0_usize, "A".to_string(), Marker::NoEbook);
        store.insert_mark(&sid, k.clone());

        assert!(store.remove_mark(&sid, &k), "first remove found it");
        assert!(!store.remove_mark(&sid, &k), "second remove is a no-op");
        assert_eq!(store.marks(&sid).len(), 0);
    }
}
