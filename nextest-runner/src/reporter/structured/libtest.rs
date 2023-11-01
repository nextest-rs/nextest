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
//! running their tests on stable instead of being forced to nightly.
//!
//! This implementation will attempt to follow the libtest format as it changes,
//! but the rate of changes is quite low (see <https://github.com/rust-lang/rust/blob/master/library/test/src/formatters/json.rs>)
//! so this should not be a big issue to users.

use super::{
    FormatVersionError, FormatVersionErrorInner, TestEvent, TestEventKind, WriteEventError,
};
use crate::runner::ExecutionResult;
use nextest_metadata::MismatchReason;
use std::collections::BTreeMap;

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
struct LibtestSuite {
    /// The number of tests that failed
    failed: usize,
    /// The number of tests that succeeded
    succeeded: usize,
    /// The number of tests that were ignored
    ignored: usize,
    /// The number of tests that were not executed due to filters
    filtered: usize,
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

/// Determines whether the `nextest` subobject is added with additional metadata
/// to events
#[derive(Copy, Clone, Debug)]
pub enum EmitNextestObject {
    /// The `nextest` subobject is added
    Yes,
    /// The `nextest` subobject is not added
    No,
}

/// A reporter that reports test runs in the same line-by-line JSON format as
/// libtest itself
pub struct LibtestReporter<'cfg> {
    _minor: FormatMinorVersion,
    _major: FormatMajorVersion,
    test_suites: BTreeMap<&'cfg str, LibtestSuite>,
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
                test_suites: BTreeMap::new(),
                emit_nextest_obj,
            });
        };
        let Some((major, minor)) = version.split_once('.') else {
            return Err(FormatVersionError {
                input: version.into(),
                err: FormatVersionErrorInner::InvalidFormat {
                    expected: "<major>.<minor>",
                },
            });
        };

        let major: u8 = major.parse().map_err(|err| FormatVersionError {
            input: version.into(),
            err: FormatVersionErrorInner::InvalidInteger {
                which: "major",
                err,
            },
        })?;

        let minor: u8 = minor.parse().map_err(|err| FormatVersionError {
            input: version.into(),
            err: FormatVersionErrorInner::InvalidInteger {
                which: "minor",
                err,
            },
        })?;

        let major = match major {
            0 => FormatMajorVersion::Unstable,
            o => {
                return Err(FormatVersionError {
                    input: version.into(),
                    err: FormatVersionErrorInner::InvalidValue {
                        which: "major",
                        value: o,
                        range: (FormatMajorVersion::Unstable as u8)
                            ..(FormatMajorVersion::_Max as u8),
                    },
                });
            }
        };

        let minor = match minor {
            0 => FormatMinorVersion::First,
            o => {
                return Err(FormatVersionError {
                    input: version.into(),
                    err: FormatVersionErrorInner::InvalidValue {
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
            test_suites: BTreeMap::new(),
            emit_nextest_obj,
        })
    }

    pub(crate) fn write_event(&mut self, event: &TestEvent<'cfg>) -> Result<(), WriteEventError> {
        use std::fmt::Write as _;

        const KIND_TEST: &str = "test";
        const KIND_SUITE: &str = "suite";

        const EVENT_STARTED: &str = "started";
        const EVENT_IGNORED: &str = "ignored";
        const EVENT_OK: &str = "ok";
        const EVENT_FAILED: &str = "failed";

        let mut retries = None;

        // Write the pieces of data that are the same across all events
        let (kind, eve, test_instance) = match &event.kind {
            TestEventKind::TestStarted { test_instance, .. } => {
                (KIND_TEST, EVENT_STARTED, test_instance)
            }
            TestEventKind::TestSkipped {
                test_instance,
                reason: MismatchReason::Ignored,
            } => {
                // Note: unfortunately, libtest does not expose the message test in `#[ignore = "<message>"]`
                // so we can't replicate the behavior of libtest excactly be emitting
                // that message as additional metadata
                (KIND_TEST, EVENT_STARTED, test_instance)
            }
            TestEventKind::TestFinished {
                test_instance,
                run_statuses,
                ..
            } => {
                if run_statuses.len() > 1 {
                    retries = Some(run_statuses.len());
                }

                (
                    KIND_TEST,
                    match run_statuses.last_status().result {
                        ExecutionResult::Pass | ExecutionResult::Leak => EVENT_OK,
                        ExecutionResult::Fail { .. }
                        | ExecutionResult::ExecFail
                        | ExecutionResult::Timeout => EVENT_FAILED,
                    },
                    test_instance,
                )
            }
            _ => return Ok(()),
        };

        #[inline]
        fn fmt_err(err: std::fmt::Error) -> WriteEventError {
            WriteEventError::Io(std::io::Error::new(std::io::ErrorKind::OutOfMemory, err))
        }

        let suite_info = test_instance.suite_info;
        let crate_name = suite_info.package.name();
        let binary_name = &suite_info.binary_name;

        // Emit the suite start if this is the first test of the suite
        let test_suite = match self.test_suites.entry(suite_info.binary_id.as_str()) {
            std::collections::btree_map::Entry::Vacant(e) => {
                let mut out = bytes::BytesMut::with_capacity(1024);
                write!(
                    &mut out,
                    r#"{{"type":"{KIND_SUITE}","event":"{EVENT_STARTED}","test_count":{}"#,
                    suite_info.status.test_count()
                )
                .map_err(fmt_err)?;

                if self.emit_nextest_obj {
                    write!(
                        &mut out,
                        r#","nextest":{{"crate":"{crate_name}","test_binary":"{binary_name}","kind":"{}"}}"#,
                        suite_info.kind,
                    )
                    .map_err(fmt_err)?;
                }

                out.extend_from_slice(b"}\n");

                e.insert(LibtestSuite {
                    failed: 0,
                    succeeded: 0,
                    ignored: 0,
                    filtered: 0,
                    total: std::time::Duration::new(0, 0),
                    ignore_block: None,
                    output_block: out,
                })
            }
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
        };

        let out = &mut test_suite.output_block;

        // After all the tests have been started or ignored, put the block of
        // tests that were ignored just as libtest does
        if matches!(event.kind, TestEventKind::TestFinished { .. }) {
            if let Some(ib) = test_suite.ignore_block.take() {
                out.extend_from_slice(&ib);
            }
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
            r#"{{"type":"{kind}","event":"{eve}","name":"{}::{}${}"#,
            suite_info.package.name(),
            suite_info.binary_name,
            test_instance.name,
        )
        .map_err(fmt_err)?;

        if let Some(retry_count) = retries {
            write!(out, "#{retry_count}\"").map_err(fmt_err)?;
        } else {
            out.extend_from_slice(b"\"");
        }

        let done = match &event.kind {
            TestEventKind::TestFinished {
                run_statuses,
                running,
                ..
            } => {
                let last_status = run_statuses.last_status();

                test_suite.total += last_status.time_taken;

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

                match last_status.result {
                    ExecutionResult::Fail { .. } | ExecutionResult::ExecFail => {
                        test_suite.failed += 1;
                        let stdout = String::from_utf8_lossy(&last_status.stdout);
                        let stderr = String::from_utf8_lossy(&last_status.stderr);

                        // TODO: Get the combined stdout and stderr streams, in the order they
                        // are supposed to be, to accurately replicate libtest's output

                        // TODO: Strip libtest stdout output
                        // libtest outputs various things when _not_ using the
                        // unstable json format that we need to strip to emulate
                        // that json output, eg.
                        //
                        // ```
                        // running <n> tests
                        // <test output>
                        // test <name> ... FAILED
                        // \n\nfailures:\n\nfailures:\n    <name>\n\ntest result: FAILED
                        // ```

                        write!(
                            out,
                            r#","stdout":"{}{}""#,
                            EscapedString(&stdout),
                            EscapedString(&stderr)
                        )
                        .map_err(fmt_err)?;
                    }
                    ExecutionResult::Timeout => {
                        test_suite.failed += 1;
                        out.extend_from_slice(br#","reason":"time limit exceeded""#);
                    }
                    _ => {
                        test_suite.succeeded += 1;
                    }
                }

                if self.emit_nextest_obj {}

                *running == 0
            }
            TestEventKind::TestSkipped { reason, .. } => {
                if matches!(reason, MismatchReason::Ignored) {
                    test_suite.ignored += 1;
                } else {
                    test_suite.filtered += 1;
                }

                if test_suite.ignore_block.is_none() {
                    test_suite.ignore_block = Some(bytes::BytesMut::with_capacity(1024));
                }

                let ib = test_suite
                    .ignore_block
                    .get_or_insert_with(|| bytes::BytesMut::with_capacity(1024));

                writeln!(
                    ib,
                    r#"{{"type":"{kind}","event":"{EVENT_IGNORED}","name":"{}::{}${}"}}"#,
                    suite_info.package.name(),
                    suite_info.binary_name,
                    test_instance.name,
                )
                .map_err(fmt_err)?;

                false
            }
            _ => false,
        };

        out.extend_from_slice(b"}\n");

        // If this is the last test of the suite, emit the test suite summary
        // before emitting the entire block
        if !done {
            return Ok(());
        }

        let event = if test_suite.failed > 0 {
            EVENT_FAILED
        } else {
            EVENT_OK
        };

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
            write!(
                out,
                r#","nextest":{{"crate":"{crate_name}","test_binary":"{binary_name}","kind":"{}"}}"#,
                suite_info.kind,
            )
            .map_err(fmt_err)?;
        }

        out.extend_from_slice(b"}\n");

        use std::io::Write as _;
        std::io::stdout()
            .write_all(out)
            .map_err(WriteEventError::Io)?;

        // Once we've emitted the output block we can remove the suite accumulator
        // to free up memory since we won't use it again
        self.test_suites.remove(suite_info.binary_id.as_str());

        Ok(())
    }
}

/// Copy of the same string escaper used in libtest
struct EscapedString<'s>(&'s str);

impl<'s> std::fmt::Display for EscapedString<'s> {
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
