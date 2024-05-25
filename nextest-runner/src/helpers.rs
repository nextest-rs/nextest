// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{list::Styles, runner::AbortStatus, write_str::WriteStr};
use camino::{Utf8Path, Utf8PathBuf};
use owo_colors::OwoColorize;
use std::{fmt, io, path::PathBuf, process::ExitStatus, time::Duration};

pub(crate) mod plural {
    pub(crate) fn setup_scripts_str(count: usize) -> &'static str {
        if count == 1 {
            "setup script"
        } else {
            "setup scripts"
        }
    }

    pub(crate) fn tests_str(count: usize) -> &'static str {
        if count == 1 {
            "test"
        } else {
            "tests"
        }
    }

    pub(crate) fn binaries_str(count: usize) -> &'static str {
        if count == 1 {
            "binary"
        } else {
            "binaries"
        }
    }

    pub(crate) fn paths_str(count: usize) -> &'static str {
        if count == 1 {
            "path"
        } else {
            "paths"
        }
    }

    pub(crate) fn files_str(count: usize) -> &'static str {
        if count == 1 {
            "file"
        } else {
            "files"
        }
    }

    pub(crate) fn directories_str(count: usize) -> &'static str {
        if count == 1 {
            "directory"
        } else {
            "directories"
        }
    }

    pub(crate) fn this_crate_str(count: usize) -> &'static str {
        if count == 1 {
            "this crate"
        } else {
            "these crates"
        }
    }

    pub(crate) fn libraries_str(count: usize) -> &'static str {
        if count == 1 {
            "library"
        } else {
            "libraries"
        }
    }
}

/// Write out a test name.
pub(crate) fn write_test_name(
    name: &str,
    style: &Styles,
    writer: &mut dyn WriteStr,
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

/// Write out a test name, `std::io::Write` version.
pub(crate) fn io_write_test_name(
    name: &str,
    style: &Styles,
    writer: &mut dyn io::Write,
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

pub(crate) fn convert_build_platform(
    platform: nextest_metadata::BuildPlatform,
) -> guppy::graph::cargo::BuildPlatform {
    match platform {
        nextest_metadata::BuildPlatform::Target => guppy::graph::cargo::BuildPlatform::Target,
        nextest_metadata::BuildPlatform::Host => guppy::graph::cargo::BuildPlatform::Host,
    }
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

/// On Windows, convert relative paths to use the main separator.
#[cfg(windows)]
pub(crate) fn convert_rel_path_to_main_sep(rel_path: &Utf8Path) -> Utf8PathBuf {
    if !rel_path.is_relative() {
        panic!(
            "path for conversion to backslash '{}' is not relative",
            rel_path
        );
    }
    rel_path.as_str().replace('/', "\\").into()
}

#[cfg(not(windows))]
pub(crate) fn convert_rel_path_to_main_sep(rel_path: &Utf8Path) -> Utf8PathBuf {
    rel_path.to_path_buf()
}

/// Join relative paths using forward slashes.
pub(crate) fn rel_path_join(rel_path: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    assert!(rel_path.is_relative(), "rel_path {rel_path} is relative");
    assert!(path.is_relative(), "path {path} is relative",);
    format!("{}/{}", rel_path, path).into()
}

#[derive(Debug)]
pub(crate) struct FormattedDuration(pub(crate) Duration);

impl fmt::Display for FormattedDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration = self.0.as_secs_f64();
        if duration > 60.0 {
            write!(f, "{}m {:.2}s", duration as u32 / 60, duration % 60.0)
        } else {
            write!(f, "{duration:.2}s")
        }
    }
}

/// Extract the abort status from an exit status.
pub(crate) fn extract_abort_status(exit_status: ExitStatus) -> Option<AbortStatus> {
    cfg_if::cfg_if! {
        if #[cfg(unix)] {
            // On Unix, extract the signal if it's found.
            use std::os::unix::process::ExitStatusExt;
            exit_status.signal().map(AbortStatus::UnixSignal)
        } else if #[cfg(windows)] {
            exit_status.code().and_then(|code| {
                (code < 0).then_some(AbortStatus::WindowsNtStatus(code))
            })
        } else {
            None
        }
    }
}

#[cfg(unix)]
pub(crate) fn signal_str(signal: i32) -> Option<&'static str> {
    // These signal numbers are the same on at least Linux, macOS and FreeBSD.
    match signal {
        1 => Some("HUP"),
        2 => Some("INT"),
        5 => Some("TRAP"),
        6 => Some("ABRT"),
        8 => Some("FPE"),
        9 => Some("KILL"),
        11 => Some("SEGV"),
        13 => Some("PIPE"),
        14 => Some("ALRM"),
        15 => Some("TERM"),
        24 => Some("XCPU"),
        25 => Some("XFSZ"),
        26 => Some("VTALRM"),
        27 => Some("PROF"),
        _ => None,
    }
}

#[cfg(windows)]
pub(crate) fn display_nt_status(nt_status: windows_sys::Win32::Foundation::NTSTATUS) -> String {
    // Convert the NTSTATUS to a Win32 error code.
    let win32_code = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(nt_status) };

    if win32_code == windows_sys::Win32::Foundation::ERROR_MR_MID_NOT_FOUND {
        // The Win32 code was not found.
        return format!("{nt_status:#x} ({nt_status})");
    }

    format!(
        "{:#x}: {}",
        nt_status,
        io::Error::from_raw_os_error(win32_code as i32)
    )
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct QuotedDisplay<'a, T: ?Sized>(pub(crate) &'a T);

impl<'a, T: ?Sized> fmt::Display for QuotedDisplay<'a, T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}'", self.0)
    }
}

// From https://twitter.com/8051Enthusiast/status/1571909110009921538
extern "C" {
    fn __nextest_external_symbol_that_does_not_exist();
}

#[inline]
#[allow(dead_code)]
pub(crate) fn statically_unreachable() -> ! {
    unsafe {
        __nextest_external_symbol_that_does_not_exist();
    }
    unreachable!("linker symbol above cannot be resolved")
}
