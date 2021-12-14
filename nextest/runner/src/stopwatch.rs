// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stopwatch for tracking how long it takes to run tests.
//!
//! Tests need to track a start time and a duration. For that we use a combination of a `SystemTime`
//! (realtime clock) and an `Instant` (monotonic clock). Once the stopwatch transitions to the "end"
//! state, we can report the elapsed time using the monotonic clock.

use std::time::{Duration, Instant, SystemTime};

/// The start state of a stopwatch.
#[derive(Clone, Debug)]
pub(crate) struct StopwatchStart {
    start_time: SystemTime,
    instant: Instant,
}

impl StopwatchStart {
    pub(crate) fn now() -> Self {
        Self {
            // These two syscalls will happen imperceptibly close to each other, which is good
            // enough for our purposes.
            start_time: SystemTime::now(),
            instant: Instant::now(),
        }
    }

    #[inline]
    pub(crate) fn elapsed(&self) -> Duration {
        self.instant.elapsed()
    }

    pub(crate) fn end(&self) -> StopwatchEnd {
        StopwatchEnd {
            start_time: self.start_time,
            duration: self.instant.elapsed(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct StopwatchEnd {
    pub(crate) start_time: SystemTime,
    pub(crate) duration: Duration,
}
