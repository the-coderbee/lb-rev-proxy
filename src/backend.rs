//! Backend module
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicI8, Ordering},
};

use crate::health::{self, CheckResult, Thresholds, Transition};

#[derive(Debug)]
/// Core backend struct respresenting a single backend object.
pub(crate) struct Backend {
    /// The socket address associated with the backend.
    address: SocketAddr,
    /// The `i8` representing the health of a backend.
    health: AtomicI8,
    /// The thresholds for a backend(fall and rise).
    thresholds: Thresholds,
}

impl Backend {
    pub(crate) fn new(address: SocketAddr, thresholds: Thresholds) -> Self {
        Self {
            address,
            health: AtomicI8::new(0),
            thresholds,
        }
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) fn is_up(&self) -> bool {
        health::is_up(self.health.load(Ordering::Relaxed))
    }

    #[must_use]
    pub(crate) fn record_success(&self) -> Option<Transition> {
        self.record(CheckResult::Success)
    }

    #[must_use]
    pub(crate) fn record_failure(&self) -> Option<Transition> {
        self.record(CheckResult::Failure)
    }

    /// A private function to record failure or success and capture backend mode transition.
    fn record(&self, result: CheckResult) -> Option<Transition> {
        let previous = self
            .health
            .update(Ordering::Relaxed, Ordering::Relaxed, |state| {
                health::next_state(state, result, &self.thresholds)
            });
        let new = health::next_state(previous, result, &self.thresholds);
        health::transition(previous, new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend with thresholds of 3 failures down and 2 successes up.
    fn test_backend() -> Backend {
        let address = "127.0.0.1:8001".parse().unwrap();
        Backend::new(address, Thresholds::new(3, 2).unwrap())
    }

    /// A backend that has just been marked Down.
    fn down_backend() -> Backend {
        let backend = test_backend();
        for _ in 0..3 {
            let _ = backend.record_failure();
        }
        assert!(!backend.is_up(), "setup: backend should be down");
        backend
    }

    #[test]
    fn new_backend_is_up() {
        let backend = test_backend();
        assert!(backend.is_up());
    }

    #[test]
    fn three_failures_mark_backend_down() {
        let backend = test_backend();

        assert_eq!(backend.record_failure(), None);
        assert_eq!(backend.record_failure(), None);
        assert_eq!(backend.record_failure(), Some(Transition::WentDown));

        assert!(!backend.is_up());
    }

    #[test]
    fn two_successes_bring_backend_up() {
        let backend = down_backend();

        assert_eq!(backend.record_success(), None);
        assert_eq!(backend.record_success(), Some(Transition::CameUp));

        assert!(backend.is_up());
    }

    #[test]
    fn success_resets_failure_streak() {
        let backend = test_backend();
        assert_eq!(backend.record_failure(), None);
        assert_eq!(backend.record_failure(), None);
        assert_eq!(backend.record_success(), None);
        assert_eq!(backend.record_failure(), None);
        assert_eq!(backend.record_failure(), None);
    }
}
