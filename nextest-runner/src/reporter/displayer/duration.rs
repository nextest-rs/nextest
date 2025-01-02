// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Display helpers for durations.

use std::{fmt, time::Duration};

pub(super) struct DisplayBracketedHhMmSs(pub(super) Duration);

impl fmt::Display for DisplayBracketedHhMmSs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This matches indicatif's elapsed_precise display.
        let total_secs = self.0.as_secs();
        let secs = total_secs % 60;
        let total_mins = total_secs / 60;
        let mins = total_mins % 60;
        let hours = total_mins / 60;

        // Buffer the output internally to provide padding.
        let out = format!("{hours:02}:{mins:02}:{secs:02}");
        write!(f, "[{:>9}] ", out)
    }
}

pub(super) struct DisplayBracketedDuration(pub(super) Duration);

impl fmt::Display for DisplayBracketedDuration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // * > means right-align.
        // * 8 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        write!(f, "[{:>8.3?}s] ", self.0.as_secs_f64())
    }
}

pub(super) struct DisplayDurationBy(pub(super) Duration);

impl fmt::Display for DisplayDurationBy {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        write!(f, "by {:>7.3?}s ", self.0.as_secs_f64())
    }
}

pub(super) struct DisplaySlowDuration(pub(super) Duration);

impl fmt::Display for DisplaySlowDuration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Inside the curly braces:
        // * > means right-align.
        // * 7 is the number of characters to pad to.
        // * .3 means print three digits after the decimal point.
        //
        // The > outside the curly braces is printed literally.
        write!(f, "[>{:>7.3?}s] ", self.0.as_secs_f64())
    }
}
