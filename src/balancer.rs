//! The balancer for distributing connections across backends.
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use thiserror::Error;

use crate::{backend::Backend, health::Thresholds};

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
    backends: Vec<Arc<Backend>>,
    counter: AtomicUsize,
}

impl RoundRobin {
    /// Creates an instance of the balancer with the given backends list.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyBackendList`] if the backends list is empty.
    pub fn new(
        addresses: Vec<SocketAddr>,
        thresholds: Thresholds,
    ) -> Result<Self, EmptyBackendList> {
        if addresses.is_empty() {
            return Err(EmptyBackendList);
        }
        let backends = addresses
            .into_iter()
            .map(|address| Arc::new(Backend::new(address, thresholds)))
            .collect();
        Ok(Self {
            backends,
            counter: AtomicUsize::new(0),
        })
    }

    /// Creates an iterator of Backend and returns it.
    ///
    /// Each call starts one position further, and yields each backend at most once.
    /// Down backends are skipped.
    /// If no backend is Up, all are yielded.
    /// Health can change during the call, so in rare cases the iterator may yield nothing.
    pub(crate) fn candidates(&self) -> impl Iterator<Item = &Backend> + '_ {
        let any_up = self.backends.iter().any(|backend| backend.is_up());
        let turn = self.counter.fetch_add(1, Ordering::Relaxed);
        let len = self.backends.len();
        let start = turn % len;
        (0..len)
            .map(move |offset| self.backends[(start + offset) % len].as_ref())
            .filter(move |backend| !any_up || backend.is_up())
    }

    /// Returns every backend, for tasks that need to own a handle to one,
    /// such as health checkers.
    pub(crate) fn backends(&self) -> &[Arc<Backend>] {
        &self.backends
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three_backend_addresses() -> Vec<SocketAddr> {
        vec![
            "127.0.0.1:8001".parse().unwrap(),
            "127.0.0.1:8002".parse().unwrap(),
            "127.0.0.1:8003".parse().unwrap(),
        ]
    }

    fn test_balancer(addresses: Vec<SocketAddr>) -> RoundRobin {
        RoundRobin::new(addresses, Thresholds::new(3, 2).unwrap()).unwrap()
    }

    fn mark_down(backend: &Backend) {
        for _ in 0..3 {
            let _ = backend.record_failure();
        }
        assert!(!backend.is_up(), "setup: backend should be down");
    }

    fn candidate_addresses(balancer: &RoundRobin) -> Vec<SocketAddr> {
        balancer
            .candidates()
            .map(|backend| backend.address())
            .collect()
    }

    #[test]
    fn candidates_yield_every_backend_once() {
        let addresses = three_backend_addresses();
        let balancer = test_balancer(addresses.clone());

        assert_eq!(candidate_addresses(&balancer), addresses);
    }

    #[test]
    fn candidates_rotate_starting_position() {
        let addresses = three_backend_addresses();
        let balancer = test_balancer(addresses.clone());

        let first_of_each_call: Vec<SocketAddr> = (0..4)
            .map(|_| balancer.candidates().next().unwrap().address())
            .collect();

        assert_eq!(
            first_of_each_call,
            [addresses[0], addresses[1], addresses[2], addresses[0]]
        );
    }

    #[test]
    fn rejects_empty_backend_list() {
        let result = RoundRobin::new(Vec::new(), Thresholds::new(3, 2).unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn candidates_skip_down_backends() {
        let addresses = three_backend_addresses();
        let balancer = test_balancer(addresses.clone());

        mark_down(&balancer.backends[1]);

        assert_eq!(candidate_addresses(&balancer), [addresses[0], addresses[2]]);
    }

    #[test]
    fn candidates_include_every_backend_when_all_are_down() {
        let addresses = three_backend_addresses();
        let balancer = test_balancer(addresses.clone());

        for backend in &balancer.backends {
            mark_down(backend);
        }

        assert_eq!(candidate_addresses(&balancer), addresses);
    }
}
