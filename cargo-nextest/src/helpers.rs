// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::output::StderrStyles;

// From https://github.com/tokio-rs/tracing/issues/2730#issuecomment-1943022805
macro_rules! dyn_event {
    ($lvl:ident, $($arg:tt)+) => {
        match $lvl {
            ::tracing::Level::TRACE => ::tracing::trace!($($arg)+),
            ::tracing::Level::DEBUG => ::tracing::debug!($($arg)+),
            ::tracing::Level::INFO => ::tracing::info!($($arg)+),
            ::tracing::Level::WARN => ::tracing::warn!($($arg)+),
            ::tracing::Level::ERROR => ::tracing::error!($($arg)+),
        }
    };
}

#[cfg(feature = "self-update")]
pub(crate) fn log_needs_update(level: tracing::Level, extra: &str, styles: &StderrStyles) {
    use owo_colors::OwoColorize;

    dyn_event!(
        level,
        "update nextest with {}{}",
        "cargo nextest self update".style(styles.bold),
        extra,
    );
}

#[cfg(not(feature = "self-update"))]
pub(crate) fn log_needs_update(level: tracing::Level, extra: &str, _styles: &StderrStyles) {
    dyn_event!(level, "update nextest via your package manager{}", extra);
}

pub(crate) const BYPASS_VERSION_TEXT: &str = ", or bypass check with --override-version-check";
