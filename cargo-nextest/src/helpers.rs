// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::output::StderrStyles;

#[cfg(feature = "self-update")]
pub(crate) fn log_needs_update(level: log::Level, extra: &str, styles: &StderrStyles) {
    use owo_colors::OwoColorize;

    log::log!(
        level,
        "update nextest with {}{}",
        "cargo nextest self update".style(styles.bold),
        extra,
    );
}

#[cfg(not(feature = "self-update"))]
pub(crate) fn log_needs_update(level: log::Level, extra: &str, _styles: &StderrStyles) {
    log::log!(level, "update nextest via your package manager{}", extra);
}

pub(crate) const BYPASS_VERSION_TEXT: &str = ", or bypass check with --override-version-check";
