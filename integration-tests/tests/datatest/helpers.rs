// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use datatest_stable::Utf8Path;
use insta::internals::SettingsBindDropGuard;

/// Binds insta settings for a test, and returns the prefix to use for snapshots.
pub(crate) fn bind_insta_settings<'a>(
    path: &'a Utf8Path,
    snapshot_path: &str,
) -> (SettingsBindDropGuard, &'a str) {
    let mut settings = insta::Settings::clone_current();
    // Make insta suitable for datatest-stable use.
    settings.set_input_file(path);
    settings.set_snapshot_path(snapshot_path);
    settings.set_prepend_module_to_snapshot(false);

    let guard = settings.bind_to_scope();
    let insta_prefix = path.file_name().unwrap();

    (guard, insta_prefix)
}
