// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(feature = "self-update")]
pub(crate) fn log_needs_update(level: log::Level, extra: &str) {
    use crate::output::SupportsColorsV2;
    use owo_colors::OwoColorize;

    log::log!(
        level,
        "update nextest with {}{}",
        "cargo nextest self update"
            .if_supports_color_2(supports_color::Stream::Stderr, |x| x.bold()),
        extra,
    );
}

#[cfg(not(feature = "self-update"))]
pub(crate) fn log_needs_update(level: log::Level, extra: &str) {
    log::log!(level, "update nextest via your package manager{}", extra);
}

pub(crate) const BYPASS_VERSION_TEXT: &str = ", or bypass check with --override-version-check";
