// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stopwatch for tracking how long it takes to run tests.
//!
//! Tests need to track a start time and a duration. For that we use a combination of a `SystemTime`
//! (realtime clock) and an `Instant` (monotonic clock). Once the stopwatch transitions to the "end"
//! state, we can report the elapsed time using the monotonic clock.

use chrono::{DateTime, Local};
use std::time::{Duration, Instant};

pub(crate) fn stopwatch() -> StopwatchStart {
    StopwatchStart::new()
}

/// The start state of a stopwatch.
#[derive(Clone, Debug)]
pub(crate) struct StopwatchStart {
    start_time: DateTime<Local>,
    instant: Instant,
    paused_time: Duration,
    pause_state: StopwatchPauseState,
}

impl StopwatchStart {
    fn new() -> Self {
        Self {
            // These two syscalls will happen imperceptibly close to each other, which is good
            // enough for our purposes.
            start_time: Local::now(),
            instant: Instant::now(),
            paused_time: Duration::ZERO,
            pause_state: StopwatchPauseState::Running,
        }
    }

    pub(crate) fn start_time(&self) -> DateTime<Local> {
        self.start_time
    }

    pub(crate) fn is_paused(&self) -> bool {
        matches!(self.pause_state, StopwatchPauseState::Paused { .. })
    }

    pub(crate) fn pause(&mut self) {
        match &self.pause_state {
            StopwatchPauseState::Running => {
                self.pause_state = StopwatchPauseState::Paused {
                    paused_at: Instant::now(),
                };
            }
            StopwatchPauseState::Paused { .. } => {
                panic!("illegal state transition: pause() called while stopwatch was paused")
            }
        }
    }

    pub(crate) fn resume(&mut self) {
        match &self.pause_state {
            StopwatchPauseState::Paused { paused_at } => {
                self.paused_time += paused_at.elapsed();
                self.pause_state = StopwatchPauseState::Running;
            }
            StopwatchPauseState::Running => {
                panic!("illegal state transition: resume() called while stopwatch was running")
            }
        }
    }

    pub(crate) fn snapshot(&self) -> StopwatchSnapshot {
        StopwatchSnapshot {
            start_time: self.start_time,
            duration: self.instant.elapsed() - self.paused_time,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct StopwatchSnapshot {
    pub(crate) start_time: DateTime<Local>,
    pub(crate) duration: Duration,
}

impl StopwatchSnapshot {
    pub(crate) fn end_time(&self) -> DateTime<Local> {
        self.start_time + self.duration
    }
}

#[derive(Clone, Debug)]
pub(crate) enum StopwatchPauseState {
    Running,
    Paused { paused_at: Instant },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopwatch_pause() {
        let mut start = stopwatch();
        let unpaused_start = start.clone();

        start.pause();
        std::thread::sleep(Duration::from_millis(250));
        start.resume();

        start.pause();
        std::thread::sleep(Duration::from_millis(300));
        start.resume();

        let end = start.snapshot();
        let unpaused_end = unpaused_start.snapshot();

        // The total time we've paused is 550ms. We can assume that unpaused_end is at least 550ms
        // greater than end. Add a a fudge factor of 100ms.
        //
        // (Previously, this used to cap the difference at 650ms, but empirically, the test would
        // sometimes fail on GitHub CI. Just setting a minimum bound is enough.)
        let difference = unpaused_end.duration - end.duration;
        assert!(
            difference > Duration::from_millis(450),
            "difference between unpaused_end and end ({difference:?}) is at least 450ms"
        )
    }
}
