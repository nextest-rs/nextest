// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Helper functions for user configuration resolution.

use super::elements::{
    CompiledRecordOverride, CompiledUiOverride, RecordOverrideData, UiOverrideData,
};
use target_spec::Platform;

/// Resolves a single setting using the standard priority order.
pub(crate) fn resolve_ui_setting<T: Clone>(
    default_value: &T,
    default_overrides: &[CompiledUiOverride],
    user_value: Option<&T>,
    user_overrides: &[CompiledUiOverride],
    host_platform: &Platform,
    get_override: impl Fn(&UiOverrideData) -> Option<&T>,
) -> T {
    // 1. User overrides (first match).
    for override_ in user_overrides {
        if override_.matches(host_platform)
            && let Some(v) = get_override(override_.data())
        {
            return v.clone();
        }
    }

    // 2. Default overrides (first match).
    for override_ in default_overrides {
        if override_.matches(host_platform)
            && let Some(v) = get_override(override_.data())
        {
            return v.clone();
        }
    }

    // 3. User base config.
    if let Some(v) = user_value {
        return v.clone();
    }

    // 4. Default base config.
    default_value.clone()
}

/// Resolves a single record setting using the standard priority order.
pub(crate) fn resolve_record_setting<T: Clone>(
    default_value: &T,
    default_overrides: &[CompiledRecordOverride],
    user_value: Option<&T>,
    user_overrides: &[CompiledRecordOverride],
    host_platform: &Platform,
    get_override: impl Fn(&RecordOverrideData) -> Option<&T>,
) -> T {
    // 1. User overrides (first match).
    for override_ in user_overrides {
        if override_.matches(host_platform)
            && let Some(v) = get_override(override_.data())
        {
            return v.clone();
        }
    }

    // 2. Default overrides (first match).
    for override_ in default_overrides {
        if override_.matches(host_platform)
            && let Some(v) = get_override(override_.data())
        {
            return v.clone();
        }
    }

    // 3. User base config.
    if let Some(v) = user_value {
        return v.clone();
    }

    // 4. Default base config.
    default_value.clone()
}
