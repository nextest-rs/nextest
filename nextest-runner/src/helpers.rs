// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! General support code for nextest-runner.

use crate::{
    config::ScriptId,
    list::{Styles, TestInstanceId},
    reporter::events::AbortStatus,
    write_str::WriteStr,
};
use camino::{Utf8Path, Utf8PathBuf};
use owo_colors::{OwoColorize, Style};
use std::{fmt, io, path::PathBuf, process::ExitStatus, time::Duration};

/// Utilities for pluralizing various words based on count or plurality.
pub mod plural {
    use crate::run_mode::NextestRunMode;

    /// Returns "were" if `plural` is true, otherwise "was".
    pub fn were_plural_if(plural: bool) -> &'static str {
        if plural { "were" } else { "was" }
    }

    /// Returns "setup script" if `count` is 1, otherwise "setup scripts".
    pub fn setup_scripts_str(count: usize) -> &'static str {
        if count == 1 {
            "setup script"
        } else {
            "setup scripts"
        }
    }

    /// Returns:
    ///
    /// * If `mode` is `Test`: "test" if `count` is 1, otherwise "tests".
    /// * If `mode` is `Benchmark`: "benchmark" if `count` is 1, otherwise "benchmarks".
    pub fn tests_str(mode: NextestRunMode, count: usize) -> &'static str {
        tests_plural_if(mode, count != 1)
    }

    /// Returns:
    ///
    /// * If `mode` is `Test`: "tests" if `plural` is true, otherwise "test".
    /// * If `mode` is `Benchmark`: "benchmarks" if `plural` is true, otherwise "benchmark".
    pub fn tests_plural_if(mode: NextestRunMode, plural: bool) -> &'static str {
        match (mode, plural) {
            (NextestRunMode::Test, true) => "tests",
            (NextestRunMode::Test, false) => "test",
            (NextestRunMode::Benchmark, true) => "benchmarks",
            (NextestRunMode::Benchmark, false) => "benchmark",
        }
    }

    /// Returns "tests" or "benchmarks" based on the run mode.
    pub fn tests_plural(mode: NextestRunMode) -> &'static str {
        match mode {
            NextestRunMode::Test => "tests",
            NextestRunMode::Benchmark => "benchmarks",
        }
    }

    /// Returns "binary" if `count` is 1, otherwise "binaries".
    pub fn binaries_str(count: usize) -> &'static str {
        if count == 1 { "binary" } else { "binaries" }
    }

    /// Returns "path" if `count` is 1, otherwise "paths".
    pub fn paths_str(count: usize) -> &'static str {
        if count == 1 { "path" } else { "paths" }
    }

    /// Returns "file" if `count` is 1, otherwise "files".
    pub fn files_str(count: usize) -> &'static str {
        if count == 1 { "file" } else { "files" }
    }

    /// Returns "directory" if `count` is 1, otherwise "directories".
    pub fn directories_str(count: usize) -> &'static str {
        if count == 1 {
            "directory"
        } else {
            "directories"
        }
    }

    /// Returns "this crate" if `count` is 1, otherwise "these crates".
    pub fn this_crate_str(count: usize) -> &'static str {
        if count == 1 {
            "this crate"
        } else {
            "these crates"
        }
    }

    /// Returns "library" if `count` is 1, otherwise "libraries".
    pub fn libraries_str(count: usize) -> &'static str {
        if count == 1 { "library" } else { "libraries" }
    }

    /// Returns "filter" if `count` is 1, otherwise "filters".
    pub fn filters_str(count: usize) -> &'static str {
        if count == 1 { "filter" } else { "filters" }
    }

    /// Returns "section" if `count` is 1, otherwise "sections".
    pub fn sections_str(count: usize) -> &'static str {
        if count == 1 { "section" } else { "sections" }
    }
}

pub(crate) struct DisplayTestInstance<'a> {
    instance: TestInstanceId<'a>,
    styles: &'a Styles,
}

impl<'a> DisplayTestInstance<'a> {
    pub(crate) fn new(instance: TestInstanceId<'a>, styles: &'a Styles) -> Self {
        Self { instance, styles }
    }
}

impl fmt::Display for DisplayTestInstance<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} ",
            self.instance.binary_id.style(self.styles.binary_id),
        )?;
        fmt_write_test_name(self.instance.test_name, self.styles, f)
    }
}

pub(crate) struct DisplayScriptInstance {
    script_id: ScriptId,
    full_command: String,
    script_id_style: Style,
}

impl DisplayScriptInstance {
    pub(crate) fn new(
        script_id: ScriptId,
        command: &str,
        args: &[String],
        script_id_style: Style,
    ) -> Self {
        let full_command =
            shell_words::join(std::iter::once(command).chain(args.iter().map(|arg| arg.as_ref())));

        Self {
            script_id,
            full_command,
            script_id_style,
        }
    }
}

impl fmt::Display for DisplayScriptInstance {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}: {}",
            self.script_id.style(self.script_id_style),
            self.full_command,
        )
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

/// Write out a test name, `std::fmt::Write` version.
pub(crate) fn fmt_write_test_name(
    name: &str,
    style: &Styles,
    writer: &mut dyn fmt::Write,
) -> fmt::Result {
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
        panic!("path for conversion to forward slash '{rel_path}' is not relative");
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
        panic!("path for conversion to backslash '{rel_path}' is not relative");
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
    format!("{rel_path}/{path}").into()
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

// "exited with"/"terminated via"
pub(crate) fn display_exited_with(exit_status: ExitStatus) -> String {
    match AbortStatus::extract(exit_status) {
        Some(abort_status) => display_abort_status(abort_status),
        None => match exit_status.code() {
            Some(code) => format!("exited with exit code {code}"),
            None => "exited with an unknown error".to_owned(),
        },
    }
}

/// Displays the abort status.
pub(crate) fn display_abort_status(abort_status: AbortStatus) -> String {
    match abort_status {
        #[cfg(unix)]
        AbortStatus::UnixSignal(sig) => match crate::helpers::signal_str(sig) {
            Some(s) => {
                format!("aborted with signal {sig} (SIG{s})")
            }
            None => {
                format!("aborted with signal {sig}")
            }
        },
        #[cfg(windows)]
        AbortStatus::WindowsNtStatus(nt_status) => {
            format!(
                "aborted with code {}",
                // TODO: pass down a style here
                crate::helpers::display_nt_status(nt_status, Style::new())
            )
        }
        #[cfg(windows)]
        AbortStatus::JobObject => "terminated via job object".to_string(),
    }
}

#[cfg(unix)]
pub(crate) fn signal_str(signal: i32) -> Option<&'static str> {
    // These signal numbers are the same on at least Linux, macOS, FreeBSD and illumos.
    //
    // TODO: glibc has sigabbrev_np, and POSIX-1.2024 adds sig2str which has been available on
    // illumos for many years:
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/sig2str.html. We should use these
    // if available.
    match signal {
        1 => Some("HUP"),
        2 => Some("INT"),
        3 => Some("QUIT"),
        4 => Some("ILL"),
        5 => Some("TRAP"),
        6 => Some("ABRT"),
        8 => Some("FPE"),
        9 => Some("KILL"),
        11 => Some("SEGV"),
        13 => Some("PIPE"),
        14 => Some("ALRM"),
        15 => Some("TERM"),
        _ => None,
    }
}

#[cfg(windows)]
pub(crate) fn display_nt_status(
    nt_status: windows_sys::Win32::Foundation::NTSTATUS,
    bold_style: Style,
) -> String {
    // 10 characters ("0x" + 8 hex digits) is how an NTSTATUS with the high bit
    // set is going to be displayed anyway. This makes all possible displays
    // uniform.
    let bolded_status = format!("{:#010x}", nt_status.style(bold_style));
    // Convert the NTSTATUS to a Win32 error code.
    let win32_code = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(nt_status) };

    if win32_code == windows_sys::Win32::Foundation::ERROR_MR_MID_NOT_FOUND {
        // The Win32 code was not found.
        return bolded_status;
    }

    format!(
        "{bolded_status}: {}",
        io::Error::from_raw_os_error(win32_code as i32)
    )
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct QuotedDisplay<'a, T: ?Sized>(pub(crate) &'a T);

impl<T: ?Sized> fmt::Display for QuotedDisplay<'_, T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}'", self.0)
    }
}

// From https://twitter.com/8051Enthusiast/status/1571909110009921538
unsafe extern "C" {
    fn __nextest_external_symbol_that_does_not_exist();
}

#[inline]
#[expect(dead_code)]
pub(crate) fn statically_unreachable() -> ! {
    unsafe {
        __nextest_external_symbol_that_does_not_exist();
    }
    unreachable!("linker symbol above cannot be resolved")
}
