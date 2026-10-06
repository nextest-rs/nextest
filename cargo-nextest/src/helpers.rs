// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::output::StderrStyles;
use nextest_runner::config::core::{ConfigSource, ConfigSourceKind, ConfigStyles};
use owo_colors::OwoColorize;
use tracing::info;

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

#[derive(Clone, Copy, Debug)]
pub(crate) enum VersionReqKind {
    Required,
    Recommended,
}

impl VersionReqKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Recommended => "recommended",
        }
    }
}

pub(crate) fn log_version_source(
    kind: VersionReqKind,
    source: &ConfigSource,
    styles: ConfigStyles,
) {
    let kind = kind.as_str();
    match source.kind() {
        // TODO: also log the config file?
        ConfigSourceKind::Tool(tool) => info!(
            target: "cargo_nextest::no_heading",
            "({kind} version specified by tool `{}`)",
            tool.style(styles.tool),
        ),
        ConfigSourceKind::ExplicitRepository | ConfigSourceKind::DiscoveredRepository => info!(
            target: "cargo_nextest::no_heading",
            "({kind} version specified in {})",
            source.path().display().style(styles.path),
        ),
    }
}
