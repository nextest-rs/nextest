// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use pin_project_lite::pin_project;
use std::{future::Future, pin::Pin, task::Poll, time::Duration};
use tokio::time::{Instant, Sleep};

pub(crate) fn pausable_sleep(duration: Duration) -> PausableSleep {
    PausableSleep::new(duration)
}

pin_project! {
    /// A wrapper around `tokio::time::Sleep` that can also be paused, resumed and reset.
    #[derive(Debug)]
    pub(crate) struct PausableSleep {
        #[pin]
        sleep: Sleep,
        duration: Duration,
        pause_state: SleepPauseState,
    }
}

impl PausableSleep {
    fn new(duration: Duration) -> Self {
        Self {
            sleep: tokio::time::sleep(duration),
            duration,
            pause_state: SleepPauseState::Running,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_paused(&self) -> bool {
        matches!(self.pause_state, SleepPauseState::Paused { .. })
    }

    pub(crate) fn pause(self: Pin<&mut Self>) {
        let this = self.project();
        match &*this.pause_state {
            SleepPauseState::Running => {
                // Figure out how long there is until the deadline.
                let deadline = this.sleep.deadline();
                this.sleep.reset(far_future());
                // This will return 0 if the deadline has passed. That's fine because we'll just
                // reset the timer back to 0 in resume, which will behave correctly.
                let remaining = deadline.duration_since(Instant::now());
                *this.pause_state = SleepPauseState::Paused { remaining };
            }
            SleepPauseState::Paused { remaining } => {
                panic!(
                    "illegal state transition: pause() called while sleep was paused (remaining = {remaining:?})"
                );
            }
        }
    }

    pub(crate) fn resume(self: Pin<&mut Self>) {
        let this = self.project();
        match &*this.pause_state {
            SleepPauseState::Paused { remaining } => {
                this.sleep.reset(Instant::now() + *remaining);
                *this.pause_state = SleepPauseState::Running;
            }
            SleepPauseState::Running => {
                panic!("illegal state transition: resume() called while sleep was running");
            }
        }
    }

    /// Resets the sleep to the given duration.
    ///
    /// * If the timer is currently running, it will be reset to
    ///   `Instant::now()` plus the last duration provided via
    ///   [`pausable_sleep`] or [`Self::reset`].
    ///
    /// * If it is currently paused, it will be reset to the new duration
    ///   whenever it is resumed.
    pub(crate) fn reset(self: Pin<&mut Self>, duration: Duration) {
        let this = self.project();
        *this.duration = duration;
        match this.pause_state {
            SleepPauseState::Running => {
                this.sleep.reset(Instant::now() + duration);
            }
            SleepPauseState::Paused { remaining } => {
                *remaining = duration;
            }
        }
    }

    /// Resets the sleep to the last duration provided.
    ///
    /// * If the timer is currently running, it will be reset to
    ///   `Instant::now()` plus the last duration provided via
    ///   [`pausable_sleep`] or [`Self::reset`].
    ///
    /// * If it is currently paused, it will be reset to the new duration
    ///   whenever it is resumed.
    pub(crate) fn reset_last_duration(self: Pin<&mut Self>) {
        let duration = self.duration;
        self.reset(duration);
    }
}

impl Future for PausableSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        // Always call into this.sleep.
        //
        // We don't do anything special for paused sleeps here. That's because
        // on pause, the sleep is reset to a far future deadline. Calling poll
        // will mean that the future gets registered with the time driver (so is
        // not going to be stuck without a waker, even though the waker will
        // never end up waking the task in practice).
        this.sleep.poll(cx)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SleepPauseState {
    Running,
    Paused { remaining: Duration },
}

// Cribbed from tokio.
fn far_future() -> Instant {
    Instant::now() + far_future_duration()
}

pub(crate) const fn far_future_duration() -> Duration {
    // Roughly 30 years from now.
    // API does not provide a way to obtain max `Instant`
    // or convert specific date in the future to instant.
    // 1000 years overflows on macOS, 100 years overflows on FreeBSD.
    Duration::from_secs(86400 * 365 * 30)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reset_on_sleep() {
        const TICK: Duration = Duration::from_millis(500);

        // Create a very short timer.
        let mut sleep = std::pin::pin!(pausable_sleep(Duration::from_millis(1)));

        // Pause the timer.
        sleep.as_mut().pause();
        assert!(
            !sleep.as_mut().sleep.is_elapsed(),
            "underlying sleep has been suspended"
        );

        // Now set the timer to one tick. This should *not* cause the timer to
        // be reset -- instead, the new timer should be buffered until the timer
        // is resumed.
        sleep.as_mut().reset(TICK);
        assert_eq!(
            sleep.as_ref().pause_state,
            SleepPauseState::Paused { remaining: TICK }
        );
        assert!(
            !sleep.as_mut().sleep.is_elapsed(),
            "underlying sleep is still suspended"
        );

        // Now sleep for 2 ticks. The timer should still be paused and not
        // completed.
        tokio::time::sleep(2 * TICK).await;
        assert!(
            !sleep.as_mut().sleep.is_elapsed(),
            "underlying sleep is still suspended after waiting 2 ticks"
        );

        // Now resume the timer and wait for it to complete. It should take
        // around 1 tick starting from this point.

        let now = Instant::now();
        sleep.as_mut().resume();
        sleep.as_mut().await;

        assert!(
            sleep.as_mut().sleep.is_elapsed(),
            "underlying sleep has finally elapsed"
        );

        assert!(now.elapsed() >= TICK);
    }
}
