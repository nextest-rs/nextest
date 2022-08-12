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
                panic!("illegal state transition: pause() called while sleep was paused (remaining = {remaining:?})");
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

    /// Resets the inner sleep to now + the original duration.
    pub(crate) fn reset_original_duration(self: Pin<&mut Self>) {
        let this = self.project();
        this.sleep.reset(Instant::now() + *this.duration);
    }
}

impl Future for PausableSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match &this.pause_state {
            SleepPauseState::Running => this.sleep.poll(cx),
            SleepPauseState::Paused { .. } => Poll::Pending,
        }
    }
}

#[derive(Debug)]
enum SleepPauseState {
    Running,
    Paused { remaining: Duration },
}

// Cribbed from tokio.
fn far_future() -> Instant {
    // Roughly 30 years from now.
    // API does not provide a way to obtain max `Instant`
    // or convert specific date in the future to instant.
    // 1000 years overflows on macOS, 100 years overflows on FreeBSD.
    Instant::now() + Duration::from_secs(86400 * 365 * 30)
}
