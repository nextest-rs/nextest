// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for core CLI argument parsing.

use super::run::filter_env_vars_for_recording;
use crate::dispatch::{app::CargoNextestApp, core::filter::TestBuildFilter};
use clap::Parser;
use nextest_runner::run_mode::NextestRunMode;
use std::collections::BTreeMap;

#[test]
fn test_argument_parsing() {
    use clap::error::ErrorKind::{self, *};

    let valid: &[&'static str] = &[
        // ---
        // Basic commands
        // ---
        "cargo nextest list",
        "cargo nextest run",
        // ---
        // Commands with arguments
        // ---
        "cargo nextest list --list-type binaries-only",
        "cargo nextest list --list-type full",
        "cargo nextest list --message-format json-pretty",
        "cargo nextest list --message-format oneline",
        "cargo nextest list --message-format auto",
        "cargo nextest list -T oneline",
        "cargo nextest list -T auto",
        "cargo nextest run --failure-output never",
        "cargo nextest run --success-output=immediate",
        "cargo nextest run --status-level=all",
        "cargo nextest run --no-capture",
        "cargo nextest run --nocapture",
        "cargo nextest run --no-run",
        "cargo nextest run --final-status-level flaky",
        "cargo nextest run --max-fail 3",
        "cargo nextest run --max-fail=all",
        // retry is an alias for flaky -- ensure that it parses
        "cargo nextest run --final-status-level retry",
        "NEXTEST_HIDE_PROGRESS_BAR=1 cargo nextest run",
        "NEXTEST_HIDE_PROGRESS_BAR=true cargo nextest run",
        // ---
        // --no-run conflicts that produce warnings rather than errors
        // ---
        "cargo nextest run --no-run -j8",
        "cargo nextest run --no-run --retries 3",
        "NEXTEST_TEST_THREADS=8 cargo nextest run --no-run",
        "cargo nextest run --no-run --success-output never",
        "NEXTEST_SUCCESS_OUTPUT=never cargo nextest run --no-run",
        "cargo nextest run --no-run --failure-output immediate",
        "NEXTEST_FAILURE_OUTPUT=immediate cargo nextest run --no-run",
        "cargo nextest run --no-run --status-level pass",
        "NEXTEST_STATUS_LEVEL=pass cargo nextest run --no-run",
        "cargo nextest run --no-run --final-status-level skip",
        "NEXTEST_FINAL_STATUS_LEVEL=skip cargo nextest run --no-run",
        // ---
        // --no-capture conflicts that produce warnings rather than errors
        // ---
        "cargo nextest run --no-capture --test-threads=24",
        "NEXTEST_NO_CAPTURE=1 cargo nextest run --test-threads=24",
        "cargo nextest run --no-capture --failure-output=never",
        "NEXTEST_NO_CAPTURE=1 cargo nextest run --failure-output=never",
        "cargo nextest run --no-capture --success-output=final",
        "NEXTEST_SUCCESS_OUTPUT=final cargo nextest run --no-capture",
        // ---
        // Cargo options
        // ---
        "cargo nextest list --lib --bins",
        "cargo nextest run --ignore-rust-version --unit-graph",
        // ---
        // Cargo message format options
        // ---
        "cargo nextest list --cargo-message-format human",
        "cargo nextest list --cargo-message-format short",
        "cargo nextest list --cargo-message-format json",
        "cargo nextest list --cargo-message-format json-diagnostic-short",
        "cargo nextest list --cargo-message-format json-diagnostic-rendered-ansi",
        "cargo nextest list --cargo-message-format json-render-diagnostics",
        "cargo nextest run --cargo-message-format json",
        // ---
        // Pager options
        // ---
        "cargo nextest list --no-pager",
        "cargo nextest show-config test-groups --no-pager",
        // ---
        // Reuse build options
        // ---
        "cargo nextest list --binaries-metadata=foo",
        "cargo nextest run --binaries-metadata=foo --target-dir-remap=bar",
        "cargo nextest list --cargo-metadata path",
        "cargo nextest run --cargo-metadata=path --workspace-remap remapped-path",
        "cargo nextest archive --archive-file my-archive.tar.zst --zstd-level -1",
        "cargo nextest archive --archive-file my-archive.foo --archive-format tar-zst",
        "cargo nextest archive --archive-file my-archive.foo --archive-format tar-zstd",
        "cargo nextest list --archive-file my-archive.tar.zst",
        "cargo nextest list --archive-file my-archive.tar.zst --archive-format tar-zst",
        "cargo nextest list --archive-file my-archive.tar.zst --extract-to my-path",
        "cargo nextest list --archive-file my-archive.tar.zst --extract-to my-path --extract-overwrite",
        "cargo nextest list --archive-file my-archive.tar.zst --persist-extract-tempdir",
        "cargo nextest list --archive-file my-archive.tar.zst --workspace-remap foo",
        "cargo nextest list --archive-file my-archive.tar.zst --config target.'cfg(all())'.runner=\"my-runner\"",
        // ---
        // Filtersets
        // ---
        "cargo nextest list -E deps(foo)",
        "cargo nextest run --filterset 'test(bar)' --package=my-package test-filter",
        "cargo nextest run --filter-expr 'test(bar)' --package=my-package test-filter",
        "cargo nextest list -E 'deps(foo)' --ignore-default-filter",
        // ---
        // Stress test options
        // ---
        "cargo nextest run --stress-count 4",
        "cargo nextest run --stress-count infinite",
        "cargo nextest run --stress-duration 60m",
        "cargo nextest run --stress-duration 24h",
        // ---
        // Test binary arguments
        // ---
        "cargo nextest run -- --a an arbitrary arg",
        // Test negative test threads
        "cargo nextest run --jobs -3",
        "cargo nextest run --jobs 3",
        // Test negative cargo build jobs
        "cargo nextest run --build-jobs -1",
        "cargo nextest run --build-jobs 1",
        // ---
        // Self update options
        // ---
        "cargo nextest self update",
        "cargo nextest self update --beta",
        "cargo nextest self update --rc",
        "cargo nextest self update --version 0.9.100",
        "cargo nextest self update --version latest",
        "cargo nextest self update --check",
        "cargo nextest self update --beta --check",
        "cargo nextest self update --rc --force",
        // ---
        // Bench command
        // ---
        "cargo nextest bench",
        "cargo nextest bench --no-run",
        "cargo nextest bench --fail-fast",
        "cargo nextest bench --no-fail-fast",
        "cargo nextest bench --max-fail 3",
        "cargo nextest bench --max-fail=all",
        "cargo nextest bench --stress-count 4",
        "cargo nextest bench --stress-count infinite",
        "cargo nextest bench --stress-duration 60m",
        "cargo nextest bench --debugger gdb",
        "cargo nextest bench --tracer strace",
        // ---
        // Replay command
        // ---
        "cargo nextest replay",
        "cargo nextest replay --run-id abc123",
        "cargo nextest replay -R abc123",
        "cargo nextest replay --exit-code",
        "cargo nextest replay --no-capture",
        "cargo nextest replay --nocapture",
        "cargo nextest replay --no-capture --failure-output never",
        "cargo nextest replay --no-capture --success-output final",
        "cargo nextest replay --no-capture --no-output-indent",
        "cargo nextest replay --status-level pass",
        "cargo nextest replay --final-status-level flaky",
    ];

    let invalid: &[(&'static str, ErrorKind)] = &[
        // ---
        // --no-run and these options conflict
        // ---
        ("cargo nextest run --no-run --fail-fast", ArgumentConflict),
        (
            "cargo nextest run --no-run --no-fail-fast",
            ArgumentConflict,
        ),
        ("cargo nextest run --no-run --max-fail=3", ArgumentConflict),
        // ---
        // --max-fail and these options conflict
        // ---
        (
            "cargo nextest run --max-fail=3 --no-fail-fast",
            ArgumentConflict,
        ),
        // ---
        // Reuse build options conflict with cargo options
        // ---
        (
            // NOTE: cargo nextest --manifest-path foo run --cargo-metadata bar is currently
            // accepted. This is a bug: https://github.com/clap-rs/clap/issues/1204
            "cargo nextest run --manifest-path foo --cargo-metadata bar",
            ArgumentConflict,
        ),
        (
            "cargo nextest run --binaries-metadata=foo --lib",
            ArgumentConflict,
        ),
        // ---
        // workspace-remap requires cargo-metadata
        // ---
        (
            "cargo nextest run --workspace-remap foo",
            MissingRequiredArgument,
        ),
        // ---
        // target-dir-remap requires binaries-metadata
        // ---
        (
            "cargo nextest run --target-dir-remap bar",
            MissingRequiredArgument,
        ),
        // ---
        // Archive options
        // ---
        (
            "cargo nextest run --archive-format tar-zst",
            MissingRequiredArgument,
        ),
        (
            "cargo nextest run --archive-file foo --archive-format no",
            InvalidValue,
        ),
        (
            "cargo nextest run --extract-to foo",
            MissingRequiredArgument,
        ),
        (
            "cargo nextest run --archive-file foo --extract-overwrite",
            MissingRequiredArgument,
        ),
        (
            "cargo nextest run --extract-to foo --extract-overwrite",
            MissingRequiredArgument,
        ),
        (
            "cargo nextest run --persist-extract-tempdir",
            MissingRequiredArgument,
        ),
        (
            "cargo nextest run --archive-file foo --extract-to bar --persist-extract-tempdir",
            ArgumentConflict,
        ),
        (
            "cargo nextest run --archive-file foo --cargo-metadata bar",
            ArgumentConflict,
        ),
        (
            "cargo nextest run --archive-file foo --binaries-metadata bar",
            ArgumentConflict,
        ),
        (
            "cargo nextest run --archive-file foo --target-dir-remap bar",
            ArgumentConflict,
        ),
        // Invalid test threads: 0
        ("cargo nextest run --jobs 0", ValueValidation),
        // Test threads must be a number
        ("cargo nextest run --jobs -twenty", UnknownArgument),
        ("cargo nextest run --build-jobs -inf1", UnknownArgument),
        // Invalid stress count: 0
        ("cargo nextest run --stress-count 0", ValueValidation),
        // Invalid stress duration: 0
        ("cargo nextest run --stress-duration 0m", ValueValidation),
        // ---
        // --debugger conflicts with stress testing and --no-run
        // ---
        (
            "cargo nextest run --debugger gdb --stress-count 4",
            ArgumentConflict,
        ),
        (
            "cargo nextest run --debugger gdb --stress-duration 1h",
            ArgumentConflict,
        ),
        (
            "cargo nextest run --debugger gdb --no-run",
            ArgumentConflict,
        ),
        // ---
        // Bench command conflicts
        // ---
        ("cargo nextest bench --no-run --fail-fast", ArgumentConflict),
        (
            "cargo nextest bench --no-run --no-fail-fast",
            ArgumentConflict,
        ),
        (
            "cargo nextest bench --no-run --max-fail=3",
            ArgumentConflict,
        ),
        (
            "cargo nextest bench --max-fail=3 --no-fail-fast",
            ArgumentConflict,
        ),
        (
            "cargo nextest bench --debugger gdb --stress-count 4",
            ArgumentConflict,
        ),
        (
            "cargo nextest bench --debugger gdb --stress-duration 1h",
            ArgumentConflict,
        ),
        (
            "cargo nextest bench --debugger gdb --no-run",
            ArgumentConflict,
        ),
        (
            "cargo nextest bench --tracer strace --stress-count 4",
            ArgumentConflict,
        ),
        // Invalid stress count: 0
        ("cargo nextest bench --stress-count 0", ValueValidation),
        // Invalid stress duration: 0
        ("cargo nextest bench --stress-duration 0m", ValueValidation),
        // ---
        // Self update option conflicts
        // ---
        ("cargo nextest self update --beta --rc", ArgumentConflict),
        (
            "cargo nextest self update --beta --version 0.9.100",
            ArgumentConflict,
        ),
        (
            "cargo nextest self update --rc --version 0.9.100",
            ArgumentConflict,
        ),
    ];

    // Unset all NEXTEST_ env vars because they can conflict with the try_parse_from below.
    for (k, _) in std::env::vars() {
        if k.starts_with("NEXTEST_") {
            // SAFETY:
            // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
            unsafe { std::env::remove_var(k) };
        }
    }

    for valid_args in valid {
        let cmd = shell_words::split(valid_args).expect("valid command line");
        // Any args in the beginning with an equals sign should be parsed as environment variables.
        let env_vars: Vec<_> = cmd
            .iter()
            .take_while(|arg| arg.contains('='))
            .cloned()
            .collect();

        let mut env_keys = Vec::with_capacity(env_vars.len());
        for k_v in &env_vars {
            let (k, v) = k_v.split_once('=').expect("valid env var");
            // SAFETY:
            // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
            unsafe { std::env::set_var(k, v) };
            env_keys.push(k);
        }

        let cmd = cmd.iter().skip(env_vars.len());

        if let Err(error) = CargoNextestApp::try_parse_from(cmd) {
            panic!("{valid_args} should have successfully parsed, but didn't: {error}");
        }

        // Unset any environment variables we set. (Don't really need to preserve the old value
        // for now.)
        for &k in &env_keys {
            // SAFETY:
            // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
            unsafe { std::env::remove_var(k) };
        }
    }

    for &(invalid_args, kind) in invalid {
        match CargoNextestApp::try_parse_from(
            shell_words::split(invalid_args).expect("valid command"),
        ) {
            Ok(_) => {
                panic!("{invalid_args} should have errored out but successfully parsed");
            }
            Err(error) => {
                let actual_kind = error.kind();
                if kind != actual_kind {
                    panic!(
                        "{invalid_args} should error with kind {kind:?}, but actual kind was {actual_kind:?}",
                    );
                }
            }
        }
    }
}

#[derive(Debug, clap::Parser)]
struct TestCli {
    #[structopt(flatten)]
    build_filter: TestBuildFilter,
}

#[test]
fn test_test_binary_argument_parsing() {
    use crate::{ExpectedError, Result};
    use nextest_runner::test_filter::{RunIgnored, TestFilterBuilder, TestFilterPatterns};

    fn get_test_filter_builder(cmd: &str) -> Result<TestFilterBuilder> {
        let app = TestCli::try_parse_from(shell_words::split(cmd).expect("valid command line"))
            .unwrap_or_else(|_| panic!("{cmd} should have successfully parsed"));
        app.build_filter
            .make_test_filter_builder(NextestRunMode::Test, vec![])
    }

    let valid = &[
        // ---
        // substring filter
        // ---
        ("foo -- str1", "foo str1"),
        ("foo -- str2 str3", "foo str2 str3"),
        // ---
        // ignored
        // ---
        ("foo -- --ignored", "foo --run-ignored only"),
        ("foo -- --ignored", "foo --run-ignored ignored-only"),
        ("foo -- --include-ignored", "foo --run-ignored all"),
        // ---
        // two escapes
        // ---
        (
            "foo -- --ignored -- str --- --ignored",
            "foo --run-ignored ignored-only str -- -- --- --ignored",
        ),
        ("foo -- -- str1 str2 --", "foo str1 str2 -- -- --"),
    ];
    let skip_exact = &[
        // ---
        // skip
        // ---
        ("foo -- --skip my-pattern --skip your-pattern", {
            let mut patterns = TestFilterPatterns::default();
            patterns.add_skip_pattern("my-pattern".to_owned());
            patterns.add_skip_pattern("your-pattern".to_owned());
            patterns
        }),
        ("foo -- pattern1 --skip my-pattern --skip your-pattern", {
            let mut patterns = TestFilterPatterns::default();
            patterns.add_substring_pattern("pattern1".to_owned());
            patterns.add_skip_pattern("my-pattern".to_owned());
            patterns.add_skip_pattern("your-pattern".to_owned());
            patterns
        }),
        // ---
        // skip and exact
        // ---
        (
            "foo -- --skip my-pattern --skip your-pattern exact1 --exact pattern2",
            {
                let mut patterns = TestFilterPatterns::default();
                patterns.add_skip_exact_pattern("my-pattern".to_owned());
                patterns.add_skip_exact_pattern("your-pattern".to_owned());
                patterns.add_exact_pattern("exact1".to_owned());
                patterns.add_exact_pattern("pattern2".to_owned());
                patterns
            },
        ),
    ];
    let invalid = &[
        // ---
        // duplicated
        // ---
        ("foo -- --include-ignored --include-ignored", "duplicated"),
        ("foo -- --ignored --ignored", "duplicated"),
        ("foo -- --exact --exact", "duplicated"),
        // ---
        // mutually exclusive
        // ---
        ("foo -- --ignored --include-ignored", "mutually exclusive"),
        ("foo --run-ignored all -- --ignored", "mutually exclusive"),
        // ---
        // missing required argument
        // ---
        ("foo -- --skip", "missing required argument"),
        // ---
        // unsupported
        // ---
        ("foo -- --bar", "unsupported"),
    ];

    for (a, b) in valid {
        let a_str = format!(
            "{:?}",
            get_test_filter_builder(a).unwrap_or_else(|_| panic!("failed to parse {a}"))
        );
        let b_str = format!(
            "{:?}",
            get_test_filter_builder(b).unwrap_or_else(|_| panic!("failed to parse {b}"))
        );
        assert_eq!(a_str, b_str);
    }

    for (args, patterns) in skip_exact {
        let builder =
            get_test_filter_builder(args).unwrap_or_else(|_| panic!("failed to parse {args}"));

        let builder2 = TestFilterBuilder::new(
            NextestRunMode::Test,
            RunIgnored::Default,
            None,
            patterns.clone(),
            Vec::new(),
        )
        .unwrap_or_else(|_| panic!("failed to build TestFilterBuilder"));

        assert!(
            builder.patterns_eq(&builder2),
            "{args} matches expected (from TestCli: {:?}, from direct construction: {:?})",
            builder,
            builder2,
        );
    }

    for (s, r) in invalid {
        let res = get_test_filter_builder(s);
        if let Err(ExpectedError::TestBinaryArgsParseError { reason, .. }) = &res {
            assert_eq!(reason, r);
        } else {
            panic!("{s} should have errored out with TestBinaryArgsParseError, actual: {res:?}",);
        }
    }
}

#[test]
fn test_filter_env_vars_for_recording() {
    let input = [
        // Should be included: NEXTEST_* and CARGO_* prefixes.
        ("NEXTEST_PROFILE", "ci"),
        ("CARGO_HOME", "/home/user/.cargo"),
        ("NEXTEST_TEST_THREADS", "4"),
        ("CARGO_TARGET_DIR", "/tmp/target"),
        // Should be excluded: ends with _TOKEN.
        ("NEXTEST_TOKEN", "secret123"),
        ("CARGO_REGISTRY_TOKEN", "crates-io-token"),
        ("NEXTEST_API_TOKEN", "api-secret"),
        // Should be excluded: neither NEXTEST_* nor CARGO_*.
        ("PATH", "/usr/bin"),
        ("HOME", "/home/user"),
        ("RUST_BACKTRACE", "1"),
        // Should be excluded: has TOKEN suffix but wrong prefix.
        ("MY_TOKEN", "other-token"),
        // Edge case: TOKEN in the middle, not at the end (should be included).
        ("NEXTEST_TOKEN_COUNT", "5"),
        ("CARGO_TOKEN_PATH", "/path"),
    ];

    let result =
        filter_env_vars_for_recording(input.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())));

    let expected: BTreeMap<String, String> = [
        ("CARGO_HOME", "/home/user/.cargo"),
        ("CARGO_TARGET_DIR", "/tmp/target"),
        ("CARGO_TOKEN_PATH", "/path"),
        ("NEXTEST_PROFILE", "ci"),
        ("NEXTEST_TEST_THREADS", "4"),
        ("NEXTEST_TOKEN_COUNT", "5"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();

    assert_eq!(result, expected);
}
