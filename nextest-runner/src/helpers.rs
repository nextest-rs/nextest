// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! General support code for nextest-runner.

use crate::{
    config::scripts::ScriptId,
    list::{OwnedTestInstanceId, Styles, TestInstanceId},
    reporter::events::{AbortStatus, StressIndex},
    run_mode::NextestRunMode,
    write_str::WriteStr,
};
use camino::{Utf8Path, Utf8PathBuf};
use console::AnsiCodeIterator;
use nextest_metadata::TestCaseName;
use owo_colors::{OwoColorize, Style};
use std::{fmt, io, ops::ControlFlow, path::PathBuf, process::ExitStatus, time::Duration};
use swrite::{SWrite, swrite};
use unicode_width::UnicodeWidthChar;

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

    /// Returns "iteration" if `count` is 1, otherwise "iterations".
    pub fn iterations_str(count: u32) -> &'static str {
        if count == 1 {
            "iteration"
        } else {
            "iterations"
        }
    }

    /// Returns "run" if `count` is 1, otherwise "runs".
    pub fn runs_str(count: usize) -> &'static str {
        if count == 1 { "run" } else { "runs" }
    }

    /// Returns "orphan" if `count` is 1, otherwise "orphans".
    pub fn orphans_str(count: usize) -> &'static str {
        if count == 1 { "orphan" } else { "orphans" }
    }

    /// Returns "error" if `count` is 1, otherwise "errors".
    pub fn errors_str(count: usize) -> &'static str {
        if count == 1 { "error" } else { "errors" }
    }

    /// Returns "exists" if `count` is 1, otherwise "exist".
    pub fn exist_str(count: usize) -> &'static str {
        if count == 1 { "exists" } else { "exist" }
    }

    /// Returns "remains" if `count` is 1, otherwise "remain".
    pub fn remains_str(count: usize) -> &'static str {
        if count == 1 { "remains" } else { "remain" }
    }
}

/// A helper for displaying test instances with formatting.
pub struct DisplayTestInstance<'a> {
    stress_index: Option<StressIndex>,
    display_counter_index: Option<DisplayCounterIndex>,
    instance: TestInstanceId<'a>,
    styles: &'a Styles,
    max_width: Option<usize>,
}

impl<'a> DisplayTestInstance<'a> {
    /// Creates a new display formatter for a test instance.
    pub fn new(
        stress_index: Option<StressIndex>,
        display_counter_index: Option<DisplayCounterIndex>,
        instance: TestInstanceId<'a>,
        styles: &'a Styles,
    ) -> Self {
        Self {
            stress_index,
            display_counter_index,
            instance,
            styles,
            max_width: None,
        }
    }

    pub(crate) fn with_max_width(mut self, max_width: usize) -> Self {
        self.max_width = Some(max_width);
        self
    }
}

impl fmt::Display for DisplayTestInstance<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Figure out the widths for each component.
        let stress_index_str = if let Some(stress_index) = self.stress_index {
            format!(
                "[{}] ",
                DisplayStressIndex {
                    stress_index,
                    count_style: self.styles.count,
                }
            )
        } else {
            String::new()
        };
        let counter_index_str = if let Some(display_counter_index) = &self.display_counter_index {
            format!("{display_counter_index} ")
        } else {
            String::new()
        };
        let binary_id_str = format!("{} ", self.instance.binary_id.style(self.styles.binary_id));
        let test_name_str = format!(
            "{}",
            DisplayTestName::new(self.instance.test_name, self.styles)
        );

        // If a max width is defined, trim strings until they fit into it.
        if let Some(max_width) = self.max_width {
            // We have to be careful while computing string width -- the strings
            // above include ANSI escape codes which have a display width of
            // zero.
            let stress_index_width = text_width(&stress_index_str);
            let counter_index_width = text_width(&counter_index_str);
            let binary_id_width = text_width(&binary_id_str);
            let test_name_width = text_width(&test_name_str);

            // Truncate components in order, from most important to keep to least:
            //
            // * stress-index (left-aligned)
            // * counter index (left-aligned)
            // * binary ID (left-aligned)
            // * test name (right-aligned)
            let mut stress_index_resolved_width = stress_index_width;
            let mut counter_index_resolved_width = counter_index_width;
            let mut binary_id_resolved_width = binary_id_width;
            let mut test_name_resolved_width = test_name_width;

            // Truncate stress-index first.
            if stress_index_resolved_width > max_width {
                stress_index_resolved_width = max_width;
            }

            // Truncate counter index next.
            let remaining_width = max_width.saturating_sub(stress_index_resolved_width);
            if counter_index_resolved_width > remaining_width {
                counter_index_resolved_width = remaining_width;
            }

            // Truncate binary ID next.
            let remaining_width = max_width
                .saturating_sub(stress_index_resolved_width)
                .saturating_sub(counter_index_resolved_width);
            if binary_id_resolved_width > remaining_width {
                binary_id_resolved_width = remaining_width;
            }

            // Truncate test name last.
            let remaining_width = max_width
                .saturating_sub(stress_index_resolved_width)
                .saturating_sub(counter_index_resolved_width)
                .saturating_sub(binary_id_resolved_width);
            if test_name_resolved_width > remaining_width {
                test_name_resolved_width = remaining_width;
            }

            // Now truncate the strings if applicable.
            let test_name_truncated_str = if test_name_resolved_width == test_name_width {
                test_name_str
            } else {
                // Right-align the test name.
                truncate_ansi_aware(
                    &test_name_str,
                    test_name_width.saturating_sub(test_name_resolved_width),
                    test_name_width,
                )
            };
            let binary_id_truncated_str = if binary_id_resolved_width == binary_id_width {
                binary_id_str
            } else {
                // Left-align the binary ID.
                truncate_ansi_aware(&binary_id_str, 0, binary_id_resolved_width)
            };
            let counter_index_truncated_str = if counter_index_resolved_width == counter_index_width
            {
                counter_index_str
            } else {
                // Left-align the counter index.
                truncate_ansi_aware(&counter_index_str, 0, counter_index_resolved_width)
            };
            let stress_index_truncated_str = if stress_index_resolved_width == stress_index_width {
                stress_index_str
            } else {
                // Left-align the stress index.
                truncate_ansi_aware(&stress_index_str, 0, stress_index_resolved_width)
            };

            write!(
                f,
                "{}{}{}{}",
                stress_index_truncated_str,
                counter_index_truncated_str,
                binary_id_truncated_str,
                test_name_truncated_str,
            )
        } else {
            write!(
                f,
                "{}{}{}{}",
                stress_index_str, counter_index_str, binary_id_str, test_name_str
            )
        }
    }
}

fn text_width(text: &str) -> usize {
    // Technically, the width of a string may not be the same as the sum of the
    // widths of its characters. But managing truncation is pretty difficult. See
    // https://docs.rs/unicode-width/latest/unicode_width/#rules-for-determining-width.
    //
    // This is quite difficult to manage truncation for, so we just use the sum
    // of the widths of the string's characters (both here and in
    // truncate_ansi_aware below).
    strip_ansi_escapes::strip_str(text)
        .chars()
        .map(|c| c.width().unwrap_or(0))
        .sum()
}

fn truncate_ansi_aware(text: &str, start: usize, end: usize) -> String {
    let mut pos = 0;
    let mut res = String::new();
    for (s, is_ansi) in AnsiCodeIterator::new(text) {
        if is_ansi {
            res.push_str(s);
            continue;
        } else if pos >= end {
            // We retain ANSI escape codes, so this is `continue` rather than
            // `break`.
            continue;
        }

        for c in s.chars() {
            let c_width = c.width().unwrap_or(0);
            if start <= pos && pos + c_width <= end {
                res.push(c);
            }
            pos += c_width;
            if pos > end {
                // no need to iterate over the rest of s
                break;
            }
        }
    }

    res
}

pub(crate) struct DisplayScriptInstance {
    stress_index: Option<StressIndex>,
    script_id: ScriptId,
    full_command: String,
    script_id_style: Style,
    count_style: Style,
}

impl DisplayScriptInstance {
    pub(crate) fn new(
        stress_index: Option<StressIndex>,
        script_id: ScriptId,
        command: &str,
        args: &[String],
        script_id_style: Style,
        count_style: Style,
    ) -> Self {
        let full_command =
            shell_words::join(std::iter::once(command).chain(args.iter().map(|arg| arg.as_ref())));

        Self {
            stress_index,
            script_id,
            full_command,
            script_id_style,
            count_style,
        }
    }
}

impl fmt::Display for DisplayScriptInstance {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(stress_index) = self.stress_index {
            write!(
                f,
                "[{}] ",
                DisplayStressIndex {
                    stress_index,
                    count_style: self.count_style,
                }
            )?;
        }
        write!(
            f,
            "{}: {}",
            self.script_id.style(self.script_id_style),
            self.full_command,
        )
    }
}

struct DisplayStressIndex {
    stress_index: StressIndex,
    count_style: Style,
}

impl fmt::Display for DisplayStressIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.stress_index.total {
            Some(total) => {
                write!(
                    f,
                    "{:>width$}/{}",
                    (self.stress_index.current + 1).style(self.count_style),
                    total.style(self.count_style),
                    width = u32_decimal_char_width(total.get()),
                )
            }
            None => {
                write!(
                    f,
                    "{}",
                    (self.stress_index.current + 1).style(self.count_style)
                )
            }
        }
    }
}

/// Counter index display for test instances.
pub enum DisplayCounterIndex {
    /// A counter with current and total counts.
    Counter {
        /// Current count.
        current: usize,
        /// Total count.
        total: usize,
    },
    /// A padded display.
    Padded {
        /// Character to use for padding.
        character: char,
        /// Width to pad to.
        width: usize,
    },
}

impl DisplayCounterIndex {
    /// Creates a new counter display.
    pub fn new_counter(current: usize, total: usize) -> Self {
        Self::Counter { current, total }
    }

    /// Creates a new padded display.
    pub fn new_padded(character: char, width: usize) -> Self {
        Self::Padded { character, width }
    }
}

impl fmt::Display for DisplayCounterIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Counter { current, total } => {
                write!(
                    f,
                    "({:>width$}/{})",
                    current,
                    total,
                    width = usize_decimal_char_width(*total)
                )
            }
            Self::Padded { character, width } => {
                // Rendered as:
                //
                // (  20/5000)
                // (---------)
                let s: String = std::iter::repeat_n(*character, 2 * *width + 1).collect();
                write!(f, "({s})")
            }
        }
    }
}

pub(crate) fn usize_decimal_char_width(n: usize) -> usize {
    // checked_ilog10 returns 0 for 1-9, 1 for 10-99, 2 for 100-999, etc. (And
    // None for 0 which we unwrap to the same as 1). Add 1 to it to get the
    // actual number of digits.
    (n.checked_ilog10().unwrap_or(0) + 1).try_into().unwrap()
}

pub(crate) fn u32_decimal_char_width(n: u32) -> usize {
    // checked_ilog10 returns 0 for 1-9, 1 for 10-99, 2 for 100-999, etc. (And
    // None for 0 which we unwrap to the same as 1). Add 1 to it to get the
    // actual number of digits.
    (n.checked_ilog10().unwrap_or(0) + 1).try_into().unwrap()
}

pub(crate) fn u64_decimal_char_width(n: u64) -> usize {
    // checked_ilog10 returns 0 for 1-9, 1 for 10-99, 2 for 100-999, etc. (And
    // None for 0 which we unwrap to the same as 1). Add 1 to it to get the
    // actual number of digits.
    (n.checked_ilog10().unwrap_or(0) + 1).try_into().unwrap()
}

/// Write out a test name.
pub(crate) fn write_test_name(
    name: &TestCaseName,
    style: &Styles,
    writer: &mut dyn WriteStr,
) -> io::Result<()> {
    let (module_path, trailing) = name.module_path_and_name();
    if let Some(module_path) = module_path {
        write!(
            writer,
            "{}{}",
            module_path.style(style.module_path),
            "::".style(style.module_path)
        )?;
    }
    write!(writer, "{}", trailing.style(style.test_name))?;

    Ok(())
}

/// Wrapper for displaying a test name with styling.
pub(crate) struct DisplayTestName<'a> {
    name: &'a TestCaseName,
    styles: &'a Styles,
}

impl<'a> DisplayTestName<'a> {
    pub(crate) fn new(name: &'a TestCaseName, styles: &'a Styles) -> Self {
        Self { name, styles }
    }
}

impl fmt::Display for DisplayTestName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (module_path, trailing) = self.name.module_path_and_name();
        if let Some(module_path) = module_path {
            write!(
                f,
                "{}{}",
                module_path.style(self.styles.module_path),
                "::".style(self.styles.module_path)
            )?;
        }
        write!(f, "{}", trailing.style(self.styles.test_name))?;

        Ok(())
    }
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

#[derive(Debug)]
pub(crate) struct FormattedRelativeDuration(pub(crate) Duration);

impl fmt::Display for FormattedRelativeDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Adapted from
        // https://github.com/atuinsh/atuin/blob/bd2a54e1b1/crates/atuin/src/command/client/search/duration.rs#L5,
        // and used under the MIT license.
        fn item(unit: &'static str, value: u64) -> ControlFlow<(&'static str, u64)> {
            if value > 0 {
                ControlFlow::Break((unit, value))
            } else {
                ControlFlow::Continue(())
            }
        }

        // impl taken and modified from
        // https://github.com/tailhook/humantime/blob/master/src/duration.rs#L295-L331
        // Copyright (c) 2016 The humantime Developers
        fn fmt(f: Duration) -> ControlFlow<(&'static str, u64), ()> {
            let secs = f.as_secs();
            let nanos = f.subsec_nanos();

            let years = secs / 31_557_600; // 365.25d
            let year_days = secs % 31_557_600;
            let months = year_days / 2_630_016; // 30.44d
            let month_days = year_days % 2_630_016;
            let days = month_days / 86400;
            let day_secs = month_days % 86400;
            let hours = day_secs / 3600;
            let minutes = day_secs % 3600 / 60;
            let seconds = day_secs % 60;

            let millis = nanos / 1_000_000;
            let micros = nanos / 1_000;

            // a difference between our impl and the original is that
            // we only care about the most-significant segment of the duration.
            // If the item call returns `Break`, then the `?` will early-return.
            // This allows for a very concise impl
            item("y", years)?;
            item("mo", months)?;
            item("d", days)?;
            item("h", hours)?;
            item("m", minutes)?;
            item("s", seconds)?;
            item("ms", u64::from(millis))?;
            item("us", u64::from(micros))?;
            item("ns", u64::from(nanos))?;
            ControlFlow::Continue(())
        }

        match fmt(self.0) {
            ControlFlow::Break((unit, value)) => write!(f, "{value}{unit}"),
            ControlFlow::Continue(()) => write!(f, "0s"),
        }
    }
}

/// Characters used for terminal output theming.
///
/// Provides both ASCII and Unicode variants for horizontal bars, progress indicators,
/// spinners, and tree display characters.
#[derive(Clone, Debug)]
pub struct ThemeCharacters {
    hbar: char,
    progress_chars: &'static str,
    use_unicode: bool,
}

impl Default for ThemeCharacters {
    fn default() -> Self {
        Self {
            hbar: '-',
            progress_chars: "=> ",
            use_unicode: false,
        }
    }
}

impl ThemeCharacters {
    /// Switches to Unicode characters for richer terminal output.
    pub fn use_unicode(&mut self) {
        self.hbar = '─';
        // https://mike42.me/blog/2018-06-make-better-cli-progress-bars-with-unicode-block-characters
        self.progress_chars = "█▉▊▋▌▍▎▏ ";
        self.use_unicode = true;
    }

    /// Returns the horizontal bar character.
    pub fn hbar_char(&self) -> char {
        self.hbar
    }

    /// Returns a horizontal bar of the specified width.
    pub fn hbar(&self, width: usize) -> String {
        std::iter::repeat_n(self.hbar, width).collect()
    }

    /// Returns the progress bar characters.
    pub fn progress_chars(&self) -> &'static str {
        self.progress_chars
    }

    /// Returns the tree branch character for non-last children: `├─` or `|-`.
    pub fn tree_branch(&self) -> &'static str {
        if self.use_unicode { "├─" } else { "|-" }
    }

    /// Returns the tree branch character for the last child: `└─` or `\-`.
    pub fn tree_last(&self) -> &'static str {
        if self.use_unicode { "└─" } else { "\\-" }
    }

    /// Returns the tree continuation line: `│ ` or `| `.
    pub fn tree_continuation(&self) -> &'static str {
        if self.use_unicode { "│ " } else { "| " }
    }

    /// Returns the tree space (no continuation): `  `.
    pub fn tree_space(&self) -> &'static str {
        "  "
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

    match windows_nt_status_message(nt_status) {
        Some(message) => format!("{bolded_status}: {message}"),
        None => bolded_status,
    }
}

/// Returns the human-readable message for a Windows NT status code, if available.
#[cfg(windows)]
pub(crate) fn windows_nt_status_message(
    nt_status: windows_sys::Win32::Foundation::NTSTATUS,
) -> Option<smol_str::SmolStr> {
    // Convert the NTSTATUS to a Win32 error code.
    let win32_code = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(nt_status) };

    if win32_code == windows_sys::Win32::Foundation::ERROR_MR_MID_NOT_FOUND {
        // The Win32 code was not found.
        return None;
    }

    Some(smol_str::SmolStr::new(
        io::Error::from_raw_os_error(win32_code as i32).to_string(),
    ))
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

/// Formats an interceptor (debugger or tracer) error message for too many tests.
pub fn format_interceptor_too_many_tests(
    cli_opt_name: &str,
    mode: NextestRunMode,
    test_count: usize,
    test_instances: &[OwnedTestInstanceId],
    list_styles: &Styles,
    count_style: Style,
) -> String {
    let mut msg = format!(
        "--{} requires exactly one {}, but {} {} were selected:",
        cli_opt_name,
        plural::tests_plural_if(mode, false),
        test_count.style(count_style),
        plural::tests_str(mode, test_count)
    );

    for test_instance in test_instances {
        let display = DisplayTestInstance::new(None, None, test_instance.as_ref(), list_styles);
        swrite!(msg, "\n  {}", display);
    }

    if test_count > test_instances.len() {
        let remaining = test_count - test_instances.len();
        swrite!(
            msg,
            "\n  ... and {} more {}",
            remaining.style(count_style),
            plural::tests_str(mode, remaining)
        );
    }

    msg
}

#[inline]
#[expect(dead_code)]
pub(crate) fn statically_unreachable() -> ! {
    unsafe {
        __nextest_external_symbol_that_does_not_exist();
    }
    unreachable!("linker symbol above cannot be resolved")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_decimal_char_width() {
        assert_eq!(1, usize_decimal_char_width(0));
        assert_eq!(1, usize_decimal_char_width(1));
        assert_eq!(1, usize_decimal_char_width(5));
        assert_eq!(1, usize_decimal_char_width(9));
        assert_eq!(2, usize_decimal_char_width(10));
        assert_eq!(2, usize_decimal_char_width(11));
        assert_eq!(2, usize_decimal_char_width(99));
        assert_eq!(3, usize_decimal_char_width(100));
        assert_eq!(3, usize_decimal_char_width(999));
    }

    #[test]
    fn test_u64_decimal_char_width() {
        assert_eq!(1, u64_decimal_char_width(0));
        assert_eq!(1, u64_decimal_char_width(1));
        assert_eq!(1, u64_decimal_char_width(9));
        assert_eq!(2, u64_decimal_char_width(10));
        assert_eq!(2, u64_decimal_char_width(99));
        assert_eq!(3, u64_decimal_char_width(100));
        assert_eq!(3, u64_decimal_char_width(999));
        assert_eq!(6, u64_decimal_char_width(999_999));
        assert_eq!(7, u64_decimal_char_width(1_000_000));
        assert_eq!(8, u64_decimal_char_width(10_000_000));
        assert_eq!(8, u64_decimal_char_width(11_000_000));
    }
}
