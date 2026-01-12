// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! libtest compatible output support
//!
//! Before 1.70.0 it was possible to send `--format json` to test executables and
//! they would print out a JSON line to stdout for various events. This format
//! was however not intended to be stabilized, so 1.70.0 made it nightly only as
//! intended. However, machine readable output is immensely useful to other
//! tooling that can much more easily consume it than parsing the output meant
//! for humans.
//!
//! Since there already existed tooling using the libtest output format, this
//! event aggregator replicates that format so that projects can seamlessly
//! integrate cargo-nextest into their project, as well as get the benefit of
//! running their tests on stable instead of being forced to use nightly.
//!
//! This implementation will attempt to follow the libtest format as it changes,
//! but the rate of changes is quite low (see <https://github.com/rust-lang/rust/blob/master/library/test/src/formatters/json.rs>)
//! so this should not be a big issue to users, however, if the format is changed,
//! the changes will be replicated in this file with a new minor version allowing
//! users to move to the new format or stick to the format version(s) they were
//! using before

use crate::{
    config::elements::{LeakTimeoutResult, SlowTimeoutResult},
    errors::{DisplayErrorChain, FormatVersionError, FormatVersionErrorInner, WriteEventError},
    list::{RustTestSuite, TestList},
    reporter::events::{
        ChildExecutionOutputDescription, ChildOutputDescription, ExecutionResultDescription,
        StressIndex, TestEvent, TestEventKind,
    },
    test_output::ChildSingleOutput,
};
use bstr::ByteSlice;
use iddqd::{IdOrdItem, IdOrdMap, id_ord_map, id_upcast};
use nextest_metadata::{MismatchReason, RustBinaryId, TestCaseName};
use std::fmt::Write as _;

/// To support pinning the version of the output, we just use this simple enum
/// to document changes as libtest output changes
#[derive(Copy, Clone)]
#[repr(u8)]
enum FormatMinorVersion {
    /// The libtest output as of `rustc 1.75.0-nightly (aa1a71e9e 2023-10-26)` with `--format json --report-time`
    ///
    /// * `{ "type": "suite", "event": "started", "test_count": <u32> }` - Start of a test binary run, always printed
    ///   * `{ "type": "test", "event": "started", "name": "<name>" }` - Start of a single test, always printed
    ///   * `{ "type": "test", "name": "<name>", "event": "ignored" }` - Printed if a test is ignored
    ///     * Will have an additional `"message" = "<message>"` field if the there is a message in the ignore attribute eg. `#[ignore = "not yet implemented"]`
    ///   * `{ "type": "test", "name": "<name>", "event": "ok", "exec_time": <f32> }` - Printed if a test runs successfully
    ///   * `{ "type": "test", "name": "<name>", "event": "failed", "exec_time": <f32>, "stdout": "<escaped output collected during test execution>" }` - Printed if a test fails, note the stdout field actually contains both stdout and stderr despite the name
    ///     * If `--ensure-time` is passed, libtest will add `"reason": "time limit exceeded"` if the test passes, but exceeds the time limit.
    ///     * If `#[should_panic = "<expected message>"]` is used and message doesn't match, an additional `"message": "panic did not contain expected string\n<panic message>"` field is added
    /// * `{ "type": "suite", "event": "<overall_status>", "passed": <u32>, "failed": <u32>, "ignored": <u32>, "measured": <u32>, "filtered_out": <u32>, "exec_time": <f32> }`
    ///   * `event` will be `"ok"` if no failures occurred, or `"failed"` if `"failed" > 0`
    ///   * `ignored` will be > 0 if there are `#[ignore]` tests and `--ignored` was not passed
    ///   * `filtered_out` with be > 0 if there were tests not marked `#[ignore]` and `--ignored` was passed OR a test filter was passed and 1 or more tests were not executed
    ///   * `measured` is only > 0 if running benchmarks
    First = 1,
    #[doc(hidden)]
    _Max,
}

/// If libtest output is ever stabilized, this would most likely become the single
/// version and we could get rid of the minor version, but who knows if that
/// will ever happen
#[derive(Copy, Clone)]
#[repr(u8)]
enum FormatMajorVersion {
    /// The libtest output is unstable
    Unstable = 0,
    #[doc(hidden)]
    _Max,
}

/// The accumulated stats for a single test binary
struct LibtestSuite<'cfg> {
    /// The number of tests that failed
    failed: usize,
    /// The number of tests that succeeded
    succeeded: usize,
    /// The number of tests that were ignored
    ignored: usize,
    /// The number of tests that were not executed due to filters
    filtered: usize,
    /// The number of tests in this suite that are still running
    running: usize,

    stress_index: Option<StressIndex>,
    meta: &'cfg RustTestSuite<'cfg>,
    /// The accumulated duration of every test that has been executed
    total: std::time::Duration,
    /// Libtest outputs outputs a `started` event for every test that isn't
    /// filtered, including ignored tests, then outputs `ignored` events after
    /// all the started events, so we just mimic that with a temporary buffer
    ignore_block: Option<bytes::BytesMut>,
    /// The single block of output accumulated for all tests executed in the binary,
    /// this needs to be emitted as a single block to emulate how cargo test works,
    /// executing each test binary serially and outputting a json line for each
    /// event, as otherwise consumers would not be able to associate a single test
    /// with its parent suite
    output_block: bytes::BytesMut,
}

impl IdOrdItem for LibtestSuite<'_> {
    type Key<'a>
        = &'a RustBinaryId
    where
        Self: 'a;

    fn key(&self) -> Self::Key<'_> {
        &self.meta.binary_id
    }

    id_upcast!();
}

/// Determines whether the `nextest` subobject is added with additional metadata
/// to events
#[derive(Copy, Clone, Debug)]
pub enum EmitNextestObject {
    /// The `nextest` subobject is added
    Yes,
    /// The `nextest` subobject is not added
    No,
}

const KIND_TEST: &str = "test";
const KIND_SUITE: &str = "suite";

const EVENT_STARTED: &str = "started";
const EVENT_IGNORED: &str = "ignored";
const EVENT_OK: &str = "ok";
const EVENT_FAILED: &str = "failed";

#[inline]
fn fmt_err(err: std::fmt::Error) -> WriteEventError {
    WriteEventError::Io(std::io::Error::new(std::io::ErrorKind::OutOfMemory, err))
}

/// A reporter that reports test runs in the same line-by-line JSON format as
/// libtest itself
pub struct LibtestReporter<'cfg> {
    _minor: FormatMinorVersion,
    _major: FormatMajorVersion,
    test_list: Option<&'cfg TestList<'cfg>>,
    test_suites: IdOrdMap<LibtestSuite<'cfg>>,
    /// If true, we emit a `nextest` subobject with additional metadata in it
    /// that consumers can use for easier integration if they wish
    emit_nextest_obj: bool,
}

impl<'cfg> LibtestReporter<'cfg> {
    /// Creates a new libtest reporter
    ///
    /// The version string is used to allow the reporter to evolve along with
    /// libtest, but still be able to output a stable format for consumers. If
    /// it is not specified the latest version of the format will be produced.
    ///
    /// If [`EmitNextestObject::Yes`] is passed, an additional `nextest` subobject
    /// will be added to some events that includes additional metadata not produced
    /// by libtest, but most consumers should still be able to consume them as
    /// the base format itself is not changed
    pub fn new(
        version: Option<&str>,
        emit_nextest_obj: EmitNextestObject,
    ) -> Result<Self, FormatVersionError> {
        let emit_nextest_obj = matches!(emit_nextest_obj, EmitNextestObject::Yes);

        let Some(version) = version else {
            return Ok(Self {
                _minor: FormatMinorVersion::First,
                _major: FormatMajorVersion::Unstable,
                test_list: None,
                test_suites: IdOrdMap::new(),
                emit_nextest_obj,
            });
        };
        let Some((major, minor)) = version.split_once('.') else {
            return Err(FormatVersionError {
                input: version.into(),
                error: FormatVersionErrorInner::InvalidFormat {
                    expected: "<major>.<minor>",
                },
            });
        };

        let major: u8 = major.parse().map_err(|err| FormatVersionError {
            input: version.into(),
            error: FormatVersionErrorInner::InvalidInteger {
                which: "major",
                err,
            },
        })?;

        let minor: u8 = minor.parse().map_err(|err| FormatVersionError {
            input: version.into(),
            error: FormatVersionErrorInner::InvalidInteger {
                which: "minor",
                err,
            },
        })?;

        let major = match major {
            0 => FormatMajorVersion::Unstable,
            o => {
                return Err(FormatVersionError {
                    input: version.into(),
                    error: FormatVersionErrorInner::InvalidValue {
                        which: "major",
                        value: o,
                        range: (FormatMajorVersion::Unstable as u8)
                            ..(FormatMajorVersion::_Max as u8),
                    },
                });
            }
        };

        let minor = match minor {
            1 => FormatMinorVersion::First,
            o => {
                return Err(FormatVersionError {
                    input: version.into(),
                    error: FormatVersionErrorInner::InvalidValue {
                        which: "minor",
                        value: o,
                        range: (FormatMinorVersion::First as u8)..(FormatMinorVersion::_Max as u8),
                    },
                });
            }
        };

        Ok(Self {
            _major: major,
            _minor: minor,
            test_list: None,
            test_suites: IdOrdMap::new(),
            emit_nextest_obj,
        })
    }

    pub(crate) fn write_event(&mut self, event: &TestEvent<'cfg>) -> Result<(), WriteEventError> {
        let mut retries = None;

        // Write the pieces of data that are the same across all events
        let (kind, eve, stress_index, test_instance) = match &event.kind {
            TestEventKind::TestStarted {
                stress_index,
                test_instance,
                ..
            } => (KIND_TEST, EVENT_STARTED, stress_index, test_instance),
            TestEventKind::TestSkipped {
                stress_index,
                test_instance,
                reason: MismatchReason::Ignored,
            } => {
                // Note: unfortunately, libtest does not expose the message test in `#[ignore = "<message>"]`
                // so we can't replicate the behavior of libtest exactly by emitting
                // that message as additional metadata
                (KIND_TEST, EVENT_STARTED, stress_index, test_instance)
            }
            TestEventKind::TestFinished {
                stress_index,
                test_instance,
                run_statuses,
                ..
            } => {
                if run_statuses.len() > 1 {
                    retries = Some(run_statuses.len());
                }

                (
                    KIND_TEST,
                    match &run_statuses.last_status().result {
                        ExecutionResultDescription::Pass
                        | ExecutionResultDescription::Timeout {
                            result: SlowTimeoutResult::Pass,
                        }
                        | ExecutionResultDescription::Leak {
                            result: LeakTimeoutResult::Pass,
                        } => EVENT_OK,
                        ExecutionResultDescription::Leak {
                            result: LeakTimeoutResult::Fail,
                        }
                        | ExecutionResultDescription::Fail { .. }
                        | ExecutionResultDescription::ExecFail
                        | ExecutionResultDescription::Timeout {
                            result: SlowTimeoutResult::Fail,
                        } => EVENT_FAILED,
                    },
                    stress_index,
                    test_instance,
                )
            }
            TestEventKind::RunStarted { test_list, .. } => {
                self.test_list = Some(*test_list);
                return Ok(());
            }
            TestEventKind::StressSubRunFinished { .. } | TestEventKind::RunFinished { .. } => {
                for test_suite in std::mem::take(&mut self.test_suites) {
                    self.finalize(test_suite)?;
                }

                return Ok(());
            }
            _ => return Ok(()),
        };

        // Look up the suite info from the test list.
        let test_list = self
            .test_list
            .expect("test_list should be set by RunStarted before any test events");
        let suite_info = test_list
            .get_suite(test_instance.binary_id)
            .expect("suite should exist in test list");
        let crate_name = suite_info.package.name();
        let binary_name = &suite_info.binary_name;

        // Emit the suite start if this is the first test of the suite
        let mut test_suite = match self.test_suites.entry(&suite_info.binary_id) {
            id_ord_map::Entry::Vacant(e) => {
                let (running, ignored, filtered) =
                    suite_info.status.test_cases().fold((0, 0, 0), |acc, case| {
                        if case.test_info.ignored {
                            (acc.0, acc.1 + 1, acc.2)
                        } else if case.test_info.filter_match.is_match() {
                            (acc.0 + 1, acc.1, acc.2)
                        } else {
                            (acc.0, acc.1, acc.2 + 1)
                        }
                    });

                let mut out = bytes::BytesMut::with_capacity(1024);
                write!(
                    &mut out,
                    r#"{{"type":"{KIND_SUITE}","event":"{EVENT_STARTED}","test_count":{}"#,
                    running + ignored,
                )
                .map_err(fmt_err)?;

                if self.emit_nextest_obj {
                    write!(
                        out,
                        r#","nextest":{{"crate":"{crate_name}","test_binary":"{binary_name}","kind":"{}""#,
                        suite_info.kind,
                    )
                    .map_err(fmt_err)?;

                    if let Some(stress_index) = stress_index {
                        write!(out, r#","stress_index":{}"#, stress_index.current)
                            .map_err(fmt_err)?;
                        if let Some(total) = stress_index.total {
                            write!(out, r#","stress_total":{total}"#).map_err(fmt_err)?;
                        }
                    }

                    write!(out, "}}").map_err(fmt_err)?;
                }

                out.extend_from_slice(b"}\n");

                e.insert(LibtestSuite {
                    running,
                    failed: 0,
                    succeeded: 0,
                    ignored,
                    filtered,
                    stress_index: *stress_index,
                    meta: suite_info,
                    total: std::time::Duration::new(0, 0),
                    ignore_block: None,
                    output_block: out,
                })
            }
            id_ord_map::Entry::Occupied(e) => e.into_mut(),
        };

        let test_suite_mut = &mut *test_suite;
        let out = &mut test_suite_mut.output_block;

        // After all the tests have been started or ignored, put the block of
        // tests that were ignored just as libtest does
        if matches!(event.kind, TestEventKind::TestFinished { .. })
            && let Some(ib) = test_suite_mut.ignore_block.take()
        {
            out.extend_from_slice(&ib);
        }

        // This is one place where we deviate from the behavior of libtest, by
        // always prefixing the test name with both the crate and the binary name,
        // as this information is quite important to distinguish tests from each
        // other when testing inside a large workspace with hundreds or thousands
        // of tests
        //
        // Additionally, a `#<n>` is used as a suffix if the test was retried,
        // as libtest does not support that functionality
        write!(
            out,
            r#"{{"type":"{kind}","event":"{eve}","name":"{}::{}"#,
            suite_info.package.name(),
            suite_info.binary_name,
        )
        .map_err(fmt_err)?;

        if let Some(stress_index) = stress_index {
            write!(out, "@stress-{}", stress_index.current).map_err(fmt_err)?;
        }
        write!(out, "${}", test_instance.test_name).map_err(fmt_err)?;
        if let Some(retry_count) = retries {
            write!(out, "#{retry_count}\"").map_err(fmt_err)?;
        } else {
            out.extend_from_slice(b"\"");
        }

        match &event.kind {
            TestEventKind::TestFinished { run_statuses, .. } => {
                let last_status = run_statuses.last_status();

                test_suite_mut.total += last_status.time_taken;
                test_suite_mut.running -= 1;

                // libtest actually requires an additional `--report-time` flag to be
                // passed for the exec_time information to be written. This doesn't
                // really make sense when outputting structured output so we emit it
                // unconditionally
                write!(
                    out,
                    r#","exec_time":{}"#,
                    last_status.time_taken.as_secs_f64()
                )
                .map_err(fmt_err)?;

                match &last_status.result {
                    ExecutionResultDescription::Fail { .. }
                    | ExecutionResultDescription::ExecFail => {
                        test_suite_mut.failed += 1;

                        // Write the output from the test into the `stdout` (even
                        // though it could contain stderr output as well).
                        write!(out, r#","stdout":""#).map_err(fmt_err)?;

                        strip_human_output_from_failed_test(
                            &last_status.output,
                            out,
                            test_instance.test_name,
                        )?;
                        out.extend_from_slice(b"\"");
                    }
                    ExecutionResultDescription::Timeout {
                        result: SlowTimeoutResult::Fail,
                    } => {
                        test_suite_mut.failed += 1;
                        out.extend_from_slice(br#","reason":"time limit exceeded""#);
                    }
                    _ => {
                        test_suite_mut.succeeded += 1;
                    }
                }
            }
            TestEventKind::TestSkipped { .. } => {
                test_suite_mut.running -= 1;

                if test_suite_mut.ignore_block.is_none() {
                    test_suite_mut.ignore_block = Some(bytes::BytesMut::with_capacity(1024));
                }

                let ib = test_suite_mut
                    .ignore_block
                    .get_or_insert_with(|| bytes::BytesMut::with_capacity(1024));

                writeln!(
                    ib,
                    r#"{{"type":"{kind}","event":"{EVENT_IGNORED}","name":"{}::{}${}"}}"#,
                    suite_info.package.name(),
                    suite_info.binary_name,
                    test_instance.test_name,
                )
                .map_err(fmt_err)?;
            }
            _ => {}
        };

        out.extend_from_slice(b"}\n");

        if self.emit_nextest_obj {
            {
                use std::io::Write as _;

                let mut stdout = std::io::stdout().lock();
                stdout.write_all(out).map_err(WriteEventError::Io)?;
                stdout.flush().map_err(WriteEventError::Io)?;
                out.clear();
            }

            if test_suite_mut.running == 0 {
                std::mem::drop(test_suite);

                if let Some(test_suite) = self.test_suites.remove(&suite_info.binary_id) {
                    self.finalize(test_suite)?;
                }
            }
        } else {
            // If this is the last test of the suite, emit the test suite summary
            // before emitting the entire block
            if test_suite_mut.running > 0 {
                return Ok(());
            }

            std::mem::drop(test_suite);

            if let Some(test_suite) = self.test_suites.remove(&suite_info.binary_id) {
                self.finalize(test_suite)?;
            }
        }

        Ok(())
    }

    fn finalize(&self, mut test_suite: LibtestSuite) -> Result<(), WriteEventError> {
        let event = if test_suite.failed > 0 {
            EVENT_FAILED
        } else {
            EVENT_OK
        };

        let out = &mut test_suite.output_block;
        let suite_info = test_suite.meta;

        // It's possible that a test failure etc has cancelled the run, in which
        // case we might still have tests that are "running", even ones that are
        // actually skipped, so we just add those to the filtered list
        if test_suite.running > 0 {
            test_suite.filtered += test_suite.running;
        }

        write!(
            out,
            r#"{{"type":"{KIND_SUITE}","event":"{event}","passed":{},"failed":{},"ignored":{},"measured":0,"filtered_out":{},"exec_time":{}"#,
            test_suite.succeeded,
            test_suite.failed,
            test_suite.ignored,
            test_suite.filtered,
            test_suite.total.as_secs_f64(),
        )
        .map_err(fmt_err)?;

        if self.emit_nextest_obj {
            let crate_name = suite_info.package.name();
            let binary_name = &suite_info.binary_name;
            write!(
                out,
                r#","nextest":{{"crate":"{crate_name}","test_binary":"{binary_name}","kind":"{}""#,
                suite_info.kind,
            )
            .map_err(fmt_err)?;

            if let Some(stress_index) = test_suite.stress_index {
                write!(out, r#","stress_index":{}"#, stress_index.current).map_err(fmt_err)?;
                if let Some(total) = stress_index.total {
                    write!(out, r#","stress_total":{total}"#).map_err(fmt_err)?;
                }
            }

            write!(out, "}}").map_err(fmt_err)?;
        }

        out.extend_from_slice(b"}\n");

        {
            use std::io::Write as _;

            let mut stdout = std::io::stdout().lock();
            stdout.write_all(out).map_err(WriteEventError::Io)?;
            stdout.flush().map_err(WriteEventError::Io)?;
        }

        Ok(())
    }
}

/// Unfortunately, to replicate the libtest json output, we need to do our own
/// filtering of the output to strip out the data emitted by libtest in the
/// human format.
///
/// This function relies on the fact that nextest runs every individual test in
/// isolation.
fn strip_human_output_from_failed_test(
    output: &ChildExecutionOutputDescription<ChildSingleOutput>,
    out: &mut bytes::BytesMut,
    test_name: &TestCaseName,
) -> Result<(), WriteEventError> {
    match output {
        ChildExecutionOutputDescription::Output {
            result: _,
            output,
            errors,
        } => {
            match output {
                ChildOutputDescription::Combined { output } => {
                    strip_human_stdout_or_combined(output, out, test_name)?;
                }
                ChildOutputDescription::Split { stdout, stderr } => {
                    // This is not a case that we hit because we always set CaptureStrategy to Combined. But
                    // handle it in a reasonable fashion. (We do have a unit test for this case, so gate the
                    // assertion with cfg(not(test)).)
                    #[cfg(not(test))]
                    {
                        debug_assert!(false, "libtest output requires CaptureStrategy::Combined");
                    }
                    if let Some(stdout) = stdout {
                        if !stdout.is_empty() {
                            write!(out, "--- STDOUT ---\\n").map_err(fmt_err)?;
                            strip_human_stdout_or_combined(stdout, out, test_name)?;
                        }
                    } else {
                        write!(out, "(stdout not captured)").map_err(fmt_err)?;
                    }
                    // If stderr is not empty, just write all of it in.
                    if let Some(stderr) = stderr {
                        if !stderr.is_empty() {
                            write!(out, "\\n--- STDERR ---\\n").map_err(fmt_err)?;
                            write!(out, "{}", EscapedString(stderr.as_str_lossy()))
                                .map_err(fmt_err)?;
                        }
                    } else {
                        writeln!(out, "\\n(stderr not captured)").map_err(fmt_err)?;
                    }
                }
            }

            if let Some(errors) = errors {
                write!(out, "\\n--- EXECUTION ERRORS ---\\n").map_err(fmt_err)?;
                write!(
                    out,
                    "{}",
                    EscapedString(&DisplayErrorChain::new(errors).to_string())
                )
                .map_err(fmt_err)?;
            }
        }
        ChildExecutionOutputDescription::StartError(error) => {
            write!(out, "--- EXECUTION ERROR ---\\n").map_err(fmt_err)?;
            write!(
                out,
                "{}",
                EscapedString(&DisplayErrorChain::new(error).to_string())
            )
            .map_err(fmt_err)?;
        }
    }
    Ok(())
}

fn strip_human_stdout_or_combined(
    output: &ChildSingleOutput,
    out: &mut bytes::BytesMut,
    test_name: &TestCaseName,
) -> Result<(), WriteEventError> {
    if output.buf.contains_str("running 1 test\n") {
        // This is most likely the default test harness.
        let lines = output
            .lines()
            .skip_while(|line| line != b"running 1 test")
            .skip(1)
            .take_while(|line| {
                if let Some(name) = line
                    .strip_prefix(b"test ")
                    .and_then(|np| np.strip_suffix(b" ... FAILED"))
                    && test_name.as_bytes() == name
                {
                    return false;
                }

                true
            })
            .map(|line| line.to_str_lossy());

        for line in lines {
            // This will never fail unless we are OOM
            write!(out, "{}\\n", EscapedString(&line)).map_err(fmt_err)?;
        }
    } else {
        // This is most likely a custom test harness. Just write out the entire
        // output.
        write!(out, "{}", EscapedString(output.as_str_lossy())).map_err(fmt_err)?;
    }

    Ok(())
}

/// Copy of the same string escaper used in libtest
///
/// <https://github.com/rust-lang/rust/blob/f440b5f0ea042cb2087a36631b20878f9847ee28/library/test/src/formatters/json.rs#L222-L285>
struct EscapedString<'s>(&'s str);

impl std::fmt::Display for EscapedString<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        let mut start = 0;
        let s = self.0;

        for (i, byte) in s.bytes().enumerate() {
            let escaped = match byte {
                b'"' => "\\\"",
                b'\\' => "\\\\",
                b'\x00' => "\\u0000",
                b'\x01' => "\\u0001",
                b'\x02' => "\\u0002",
                b'\x03' => "\\u0003",
                b'\x04' => "\\u0004",
                b'\x05' => "\\u0005",
                b'\x06' => "\\u0006",
                b'\x07' => "\\u0007",
                b'\x08' => "\\b",
                b'\t' => "\\t",
                b'\n' => "\\n",
                b'\x0b' => "\\u000b",
                b'\x0c' => "\\f",
                b'\r' => "\\r",
                b'\x0e' => "\\u000e",
                b'\x0f' => "\\u000f",
                b'\x10' => "\\u0010",
                b'\x11' => "\\u0011",
                b'\x12' => "\\u0012",
                b'\x13' => "\\u0013",
                b'\x14' => "\\u0014",
                b'\x15' => "\\u0015",
                b'\x16' => "\\u0016",
                b'\x17' => "\\u0017",
                b'\x18' => "\\u0018",
                b'\x19' => "\\u0019",
                b'\x1a' => "\\u001a",
                b'\x1b' => "\\u001b",
                b'\x1c' => "\\u001c",
                b'\x1d' => "\\u001d",
                b'\x1e' => "\\u001e",
                b'\x1f' => "\\u001f",
                b'\x7f' => "\\u007f",
                _ => {
                    continue;
                }
            };

            if start < i {
                f.write_str(&s[start..i])?;
            }

            f.write_str(escaped)?;

            start = i + 1;
        }

        if start != self.0.len() {
            f.write_str(&s[start..])?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::{
        errors::ChildStartError,
        reporter::{
            events::ChildExecutionOutputDescription,
            structured::libtest::strip_human_output_from_failed_test,
        },
        test_output::{ChildExecutionOutput, ChildOutput, ChildSplitOutput},
    };
    use bytes::BytesMut;
    use color_eyre::eyre::eyre;
    use nextest_metadata::TestCaseName;
    use std::{io, sync::Arc};

    /// Validates that the human output portion from a failed test is stripped
    /// out when writing a JSON string, as it is not part of the output when
    /// libtest itself outputs the JSON, so we have 100% identical output to libtest
    #[test]
    fn strips_human_output() {
        const TEST_OUTPUT: &[&str] = &[
            "\n",
            "running 1 test\n",
            "[src/index.rs:185] \"boop\" = \"boop\"\n",
            "this is stdout\n",
            "this i stderr\nok?\n",
            "thread 'index::test::download_url_crates_io'",
            r" panicked at src/index.rs:206:9:
oh no
stack backtrace:
    0: rust_begin_unwind
                at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/std/src/panicking.rs:597:5
    1: core::panicking::panic_fmt
                at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/core/src/panicking.rs:72:14
    2: tame_index::index::test::download_url_crates_io
                at ./src/index.rs:206:9
    3: tame_index::index::test::download_url_crates_io::{{closure}}
                at ./src/index.rs:179:33
    4: core::ops::function::FnOnce::call_once
                at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/core/src/ops/function.rs:250:5
    5: core::ops::function::FnOnce::call_once
                at /rustc/a28077b28a02b92985b3a3faecf92813155f1ea1/library/core/src/ops/function.rs:250:5
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
",
            "test index::test::download_url_crates_io ... FAILED\n",
            "\n\nfailures:\n\nfailures:\n    index::test::download_url_crates_io\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 13 filtered out; finished in 0.01s\n",
        ];

        let output = {
            let mut acc = BytesMut::new();
            for line in TEST_OUTPUT {
                acc.extend_from_slice(line.as_bytes());
            }

            ChildOutput::Combined {
                output: acc.freeze().into(),
            }
        };

        let mut actual = bytes::BytesMut::new();
        let output_desc: ChildExecutionOutputDescription<_> = ChildExecutionOutput::Output {
            result: None,
            output,
            errors: None,
        }
        .into();
        strip_human_output_from_failed_test(
            &output_desc,
            &mut actual,
            &TestCaseName::new("index::test::download_url_crates_io"),
        )
        .unwrap();

        insta::assert_snapshot!(std::str::from_utf8(&actual).unwrap());
    }

    #[test]
    fn strips_human_output_custom_test_harness() {
        // For a custom test harness, we don't strip the human output at all.
        const TEST_OUTPUT: &[&str] = &["\n", "this is a custom test harness!!!\n", "1 test passed"];

        let output = {
            let mut acc = BytesMut::new();
            for line in TEST_OUTPUT {
                acc.extend_from_slice(line.as_bytes());
            }

            ChildOutput::Combined {
                output: acc.freeze().into(),
            }
        };

        let mut actual = bytes::BytesMut::new();
        let output_desc: ChildExecutionOutputDescription<_> = ChildExecutionOutput::Output {
            result: None,
            output,
            errors: None,
        }
        .into();
        strip_human_output_from_failed_test(
            &output_desc,
            &mut actual,
            &TestCaseName::new("non-existent"),
        )
        .unwrap();

        insta::assert_snapshot!(std::str::from_utf8(&actual).unwrap());
    }

    #[test]
    fn strips_human_output_start_error() {
        let inner_error = eyre!("inner error");
        let error = io::Error::other(inner_error);

        let output: ChildExecutionOutputDescription<_> =
            ChildExecutionOutput::StartError(ChildStartError::Spawn(Arc::new(error))).into();

        let mut actual = bytes::BytesMut::new();
        strip_human_output_from_failed_test(
            &output,
            &mut actual,
            &TestCaseName::new("non-existent"),
        )
        .unwrap();

        insta::assert_snapshot!(std::str::from_utf8(&actual).unwrap());
    }

    #[test]
    fn strips_human_output_none() {
        let mut actual = bytes::BytesMut::new();
        let output_desc: ChildExecutionOutputDescription<_> = ChildExecutionOutput::Output {
            result: None,
            output: ChildOutput::Split(ChildSplitOutput {
                stdout: None,
                stderr: None,
            }),
            errors: None,
        }
        .into();
        strip_human_output_from_failed_test(
            &output_desc,
            &mut actual,
            &TestCaseName::new("non-existent"),
        )
        .unwrap();

        insta::assert_snapshot!(std::str::from_utf8(&actual).unwrap());
    }
}
