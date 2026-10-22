//! The balancer for distributing connections across backends.
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicUsize, Ordering},
};

use thiserror::Error;

/// Error that can occur while constructing balancer if the
/// provided backends list is empty.
#[derive(Debug, Error)]
#[error("backend list is empty")]
pub struct EmptyBackendList;

/// Distributes connections across backends in round-robin order.
///
/// Always holds at least one backend: [`RoundRobin::new`] rejects an
/// empty list. Safe to share between tasks (for example in an `Arc`);
/// selection uses an atomic counter, so concurrent callers never wait
/// on each other.
pub struct RoundRobin {
    backends: Vec<SocketAddr>,
    counter: AtomicUsize,
}

impl RoundRobin {
    /// Creates an instance of the balancer with the given backends list.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyBackendList`] if the backends list is empty.
    pub fn new(backends: Vec<SocketAddr>) -> Result<Self, EmptyBackendList> {
        if backends.is_empty() {
            return Err(EmptyBackendList);
        }
        Ok(Self {
            backends,
            counter: AtomicUsize::new(0),
        })
    }

    /// Creates an iterator of SocketAddress and returns it.
    pub fn candidates(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        let turn = self.counter.fetch_add(1, Ordering::Relaxed);
        let len = self.backends.len();
        let start = turn % len;

        (0..len).map(move |offset| self.backends[(start + offset) % len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three_backends() -> Vec<SocketAddr> {
        vec![
            "127.0.0.1:8001".parse().unwrap(),
            "127.0.0.1:8002".parse().unwrap(),
            "127.0.0.1:8003".parse().unwrap(),
        ]
    }
    #[test]
    fn candidates_yield_every_backend_once() {
        let backends = three_backends();
        let balancer = RoundRobin::new(backends.clone()).unwrap();

        let candidates: Vec<SocketAddr> = balancer.candidates().collect();

        assert_eq!(candidates, backends);
    }

    #[test]
    fn candidates_rotate_starting_position() {
        let backends = three_backends();
        let balancer = RoundRobin::new(backends.clone()).unwrap();
        let b1 = balancer.candidates().next().unwrap();
        let b2 = balancer.candidates().next().unwrap();
        let b3 = balancer.candidates().next().unwrap();
        let b4 = balancer.candidates().next().unwrap();

        assert_eq!(b1, backends[0]);
        assert_eq!(b2, backends[1]);
        assert_eq!(b3, backends[2]);
        assert_eq!(b4, backends[0]);
    }

    #[test]
    fn rejects_empty_backend_list() {
        let result = RoundRobin::new(Vec::new());
        assert!(result.is_err());
    }
}
