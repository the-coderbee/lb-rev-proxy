//! Health check module
use thiserror::Error;

#[derive(Debug, Error)]
#[error("health thresholds must be between 1 and 127")]
pub struct InvalidThresholds;

/// Enum for check result.
///
/// It tells if a connection succeeded or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckResult {
    Success,
    Failure,
}

/// Threshold contains the min and max values for fall and rise
/// which determines how many consecutive failures or successes
/// can happen and when they trigger a mode change.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Number of failures before the mode is switched to Down.
    fall: i8,
    /// Number of success after which the mode is switched to Up.
    rise: i8,
}

impl Thresholds {
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

/// A helper function that tells if a backend is up or down.
pub(crate) fn is_up(state: i8) -> bool {
    state >= 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_trace(start: i8, thresholds: &Thresholds, trace: &[(CheckResult, i8)]) {
        let mut state = start;
        for (step, &(event, expected)) in trace.into_iter().enumerate() {
            state = next_state(state, event, &thresholds);
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
}
