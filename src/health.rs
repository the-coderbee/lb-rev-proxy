//! Rules for how a backend's health state changes.
//!
//! A backend's health is a single `i8` that records both its mode (Up or
//! Down) and its current streak of consecutive results. [`next_state`]
//! applies one check result to a state, and [`transition`] reports whether
//! a change of state also changed the mode. Everything here is pure logic,
//! with no atomics or I/O, so the rules can be tested exhaustively.
use thiserror::Error;

/// Error returned when a health threshold is outside the valid range.
#[derive(Debug, Error)]
#[error("health thresholds must be between 1 and 127")]
pub struct InvalidThresholds;

/// The outcome of one health check or one connection attempt to a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckResult {
    /// The backend accepted the connection.
    Success,
    /// The connection attempt failed or timed out.
    Failure,
}

/// A change of a backend's mode caused by a check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transition {
    /// The backend switched from Up to Down.
    WentDown,
    /// The backend switched from Down to Up.
    CameUp,
}

/// How many consecutive results it takes to change a backend's mode.
///
/// Both values are between 1 and 127; [`Thresholds::new`] rejects anything
/// else, which keeps the state arithmetic in [`next_state`] within `i8`.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Consecutive failures that switch an Up backend to Down.
    fall: i8,
    /// Consecutive successes that switch a Down backend to Up.
    rise: i8,
}

impl Thresholds {
    /// Creates thresholds from the number of consecutive failures (`fall`)
    /// and consecutive successes (`rise`) needed to change mode.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidThresholds`] if either value is 0 or greater than 127.
    pub fn new(fall: u8, rise: u8) -> Result<Self, InvalidThresholds> {
        if fall == 0 || rise == 0 {
            return Err(InvalidThresholds);
        }
        let fall = i8::try_from(fall).map_err(|_| InvalidThresholds)?;
        let rise = i8::try_from(rise).map_err(|_| InvalidThresholds)?;
        Ok(Self { fall, rise })
    }
}

/// Returns a backend's health state after one check result.
///
/// The state is a single `i8`:
/// - `0` or positive: the backend is **Up**, and the value is the number
///   of consecutive failures. Reaching `fall` failures switches it Down.
/// - negative: the backend is **Down**, and `-state - 1` is the number of
///   consecutive successes. Reaching `rise` successes switches it Up.
///
/// A result that breaks a streak resets it: a success while Up returns `0`,
/// and a failure while Down returns `-1`. A switch to Down lands on `-1`,
/// and a switch to Up lands on `0`.
pub(crate) fn next_state(state: i8, result: CheckResult, thresholds: &Thresholds) -> i8 {
    if state >= 0 {
        match result {
            CheckResult::Success => 0,
            CheckResult::Failure if state + 1 >= thresholds.fall => -1,
            CheckResult::Failure => state + 1,
        }
    } else {
        match result {
            CheckResult::Failure => -1,
            CheckResult::Success if -state >= thresholds.rise => 0,
            CheckResult::Success => state - 1,
        }
    }
}

/// Returns `true` if `state` means the backend is Up.
pub(crate) fn is_up(state: i8) -> bool {
    state >= 0
}

/// Returns the mode change between two health states, if any.
///
/// Only the mode matters, not the streak: moving from one Up state to
/// another (or one Down state to another) returns `None`.
pub(crate) fn transition(previous: i8, new: i8) -> Option<Transition> {
    match (is_up(previous), is_up(new)) {
        (true, false) => Some(Transition::WentDown),
        (false, true) => Some(Transition::CameUp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies each event in `trace` to `start`, checking the state after
    /// every step.
    fn assert_trace(start: i8, thresholds: &Thresholds, trace: &[(CheckResult, i8)]) {
        let mut state = start;
        for (step, &(event, expected)) in trace.iter().enumerate() {
            state = next_state(state, event, thresholds);
            assert_eq!(state, expected, "wrong state after step {}", step + 1);
        }
    }

    #[test]
    fn follows_streak_rules_through_full_trace() {
        let thresholds = Thresholds::new(3, 2).unwrap();
        assert_trace(
            0,
            &thresholds,
            &[
                (CheckResult::Failure, 1),
                (CheckResult::Failure, 2),
                (CheckResult::Success, 0),
                (CheckResult::Failure, 1),
                (CheckResult::Failure, 2),
                (CheckResult::Failure, -1),
                (CheckResult::Success, -2),
                (CheckResult::Failure, -1),
                (CheckResult::Success, -2),
                (CheckResult::Success, 0),
            ],
        );
    }

    #[test]
    fn failure_while_down_without_successes_stays_at_reset() {
        let thresholds = Thresholds::new(3, 2).unwrap();
        assert_trace(
            2,
            &thresholds,
            &[
                (CheckResult::Success, 0),
                (CheckResult::Failure, 1),
                (CheckResult::Failure, 2),
                (CheckResult::Failure, -1),
                (CheckResult::Failure, -1),
                (CheckResult::Success, -2),
                (CheckResult::Success, 0),
            ],
        );
    }

    #[test]
    fn fall_of_one_switches_down_on_first_failure() {
        // Guards the `state + 1 >= fall` boundary: one failure from a
        // clean Up state must switch straight to Down.
        let thresholds = Thresholds::new(1, 2).unwrap();
        assert_trace(0, &thresholds, &[(CheckResult::Failure, -1)]);
    }

    #[test]
    fn rise_of_one_switches_up_on_first_success() {
        // Guards the `-state >= rise` boundary: one success from a fresh
        // Down state must switch straight back Up.
        let thresholds = Thresholds::new(3, 1).unwrap();
        assert_trace(-1, &thresholds, &[(CheckResult::Success, 0)]);
    }

    #[test]
    fn thresholds_reject_zero() {
        assert!(Thresholds::new(0, 2).is_err());
        assert!(Thresholds::new(3, 0).is_err());
    }

    #[test]
    fn thresholds_reject_values_above_i8_range() {
        assert!(Thresholds::new(128, 2).is_err());
        assert!(Thresholds::new(3, 128).is_err());
    }

    #[test]
    fn thresholds_accept_range_limits() {
        assert!(Thresholds::new(1, 1).is_ok());
        assert!(Thresholds::new(127, 127).is_ok());
    }

    #[test]
    fn transition_reports_switch_to_down() {
        assert_eq!(transition(2, -1), Some(Transition::WentDown));
    }

    #[test]
    fn transition_reports_switch_to_up() {
        assert_eq!(transition(-2, 0), Some(Transition::CameUp));
    }

    #[test]
    fn transition_ignores_streak_changes_while_up() {
        assert_eq!(transition(0, 1), None);
        assert_eq!(transition(1, 0), None);
    }

    #[test]
    fn transition_ignores_streak_changes_while_down() {
        assert_eq!(transition(-1, -2), None);
        assert_eq!(transition(-2, -1), None);
    }
}
