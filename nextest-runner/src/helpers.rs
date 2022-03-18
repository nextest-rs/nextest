// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::list::Styles;
use camino::{Utf8Path, Utf8PathBuf};
use owo_colors::OwoColorize;
use std::{
    io::{self, Write},
    path::PathBuf,
};

/// Write out a test name.
pub(crate) fn write_test_name(
    name: &str,
    style: &Styles,
    mut writer: impl Write,
) -> io::Result<()> {
    // Look for the part of the test after the last ::, if any.
    let mut splits = name.rsplitn(2, "::");
    let trailing = splits.next().expect("test should have at least 1 element");
    if let Some(rest) = splits.next() {
        write!(
            writer,
            "{}{}",
            rest.style(style.module_path),
            "::".style(style.module_path)
        )?;
    }
    write!(writer, "{}", trailing.style(style.test_name))?;

    Ok(())
}

// ---
// Functions below copied from cargo-util to avoid pulling in a bunch of dependencies
// ---

/// Returns the name of the environment variable used for searching for
/// dynamic libraries.
pub(crate) fn dylib_path_envvar() -> &'static str {
    if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        // When loading and linking a dynamic library or bundle, dlopen
        // searches in LD_LIBRARY_PATH, DYLD_LIBRARY_PATH, PWD, and
        // DYLD_FALLBACK_LIBRARY_PATH.
        // In the Mach-O format, a dynamic library has an "install path."
        // Clients linking against the library record this path, and the
        // dynamic linker, dyld, uses it to locate the library.
        // dyld searches DYLD_LIBRARY_PATH *before* the install path.
        // dyld searches DYLD_FALLBACK_LIBRARY_PATH only if it cannot
        // find the library in the install path.
        // Setting DYLD_LIBRARY_PATH can easily have unintended
        // consequences.
        //
        // Also, DYLD_LIBRARY_PATH appears to have significant performance
        // penalty starting in 10.13. Cargo's testsuite ran more than twice as
        // slow with it on CI.
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    }
}

/// Returns a list of directories that are searched for dynamic libraries.
///
/// Note that some operating systems will have defaults if this is empty that
/// will need to be dealt with.
pub(crate) fn dylib_path() -> Vec<PathBuf> {
    match std::env::var_os(dylib_path_envvar()) {
        Some(var) => std::env::split_paths(&var).collect(),
        None => Vec::new(),
    }
}

/// On Windows, convert relative paths to always use forward slashes.
#[cfg(windows)]
pub(crate) fn convert_rel_path_to_forward_slash(rel_path: &Utf8Path) -> Utf8PathBuf {
    if !rel_path.is_relative() {
        panic!(
            "path for conversion to forward slash '{}' is not relative",
            rel_path
        );
    }
    rel_path.as_str().replace('\\', "/").into()
}

#[cfg(not(windows))]
pub(crate) fn convert_rel_path_to_forward_slash(rel_path: &Utf8Path) -> Utf8PathBuf {
    rel_path.to_path_buf()
}
