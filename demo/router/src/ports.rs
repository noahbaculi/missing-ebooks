//! A fixed pool of loopback ports handed out to sandboxes. Allocation hands back
//! the lowest free port; release returns it for reuse. The pool is the single
//! source of truth for which ports are in flight, so the proxy never has to
//! probe the OS for a free port.

use std::collections::BTreeSet;

/// A bounded set of free ports within an inclusive range.
#[derive(Debug)]
pub struct PortPool {
    free: BTreeSet<u16>,
}

impl PortPool {
    /// Build a pool over the inclusive range `[low, high]`.
    pub fn new(low: u16, high: u16) -> Self {
        Self {
            free: (low..=high).collect(),
        }
    }

    /// Take the lowest free port, or `None` when the pool is exhausted.
    pub fn allocate(&mut self) -> Option<u16> {
        let port = *self.free.iter().next()?;
        self.free.remove(&port);
        Some(port)
    }

    /// Return a port to the pool. Releasing a port that is already free or out
    /// of range is a no-op, which keeps double-release from corrupting the set.
    pub fn release(&mut self, port: u16) {
        self.free.insert(port);
    }

    /// How many ports remain available.
    pub fn available(&self) -> usize {
        self.free.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_lowest_first_then_exhausts() {
        let mut pool = PortPool::new(9000, 9001);
        assert_eq!(pool.allocate(), Some(9000));
        assert_eq!(pool.allocate(), Some(9001));
        assert_eq!(pool.allocate(), None);
        assert_eq!(pool.available(), 0);
    }

    #[test]
    fn released_port_is_reusable() {
        let mut pool = PortPool::new(9000, 9001);
        let a = pool.allocate().unwrap();
        pool.release(a);
        assert_eq!(pool.allocate(), Some(9000));
    }
}
