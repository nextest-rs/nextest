// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A wrapper script used to run tests.
//!
//! This script outputs information to standard error, which is then captured by
//! nextest's tests.

use std::{env, fs, path::PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    eprintln!("[wrapper] args: {args:?}");

    // Expects the command line environment variable to be set.
    let _ = std::env::var("WRAPPER_CMD_ENV_VAR").expect("WRAPPER_CMD_ENV_VAR set by command.env");

    // If this is the list phase, also produce a fake test name.
    let phase = std::env::var("NEXTEST_TEST_PHASE").expect("NEXTEST_TEST_PHASE must be set");
    if phase == "list" {
        // The list phase exposes a subset of the run-context environment
        // variables.
        for var in [
            "NEXTEST_RUN_ID",
            "NEXTEST_BINARY_ID",
            "NEXTEST_WORKSPACE_ROOT",
            "NEXTEST_VERSION",
            "NEXTEST_REQUIRED_VERSION",
            "NEXTEST_RECOMMENDED_VERSION",
        ] {
            let value = std::env::var(var)
                .unwrap_or_else(|_| panic!("{} is set during the list phase", var));
            assert!(
                !value.is_empty(),
                "{} is non-empty during the list phase",
                var
            );
        }

        println!("fake_test_name: test");
    }

    // Execute the test binary with the arguments.
    let status = std::process::Command::new(&args[2])
        .args(&args[3..])
        .status()
        .expect("failed to execute test binary");

    let mut code = status.code().unwrap_or(1);
    if phase == "run" {
        if let Ok(mode) = env::var("__NEXTEST_WRAPPER_REPORT_MODE") {
            let path =
                PathBuf::from(env::var_os("NEXTEST_RUN_WRAPPER_REPORT").expect("report path"));
            assert_eq!(path.file_name().unwrap(), "report.json");
            assert!(!path.exists(), "each attempt starts without a report");
            let audit =
                PathBuf::from(env::var_os("__NEXTEST_WRAPPER_REPORT_AUDIT").expect("audit dir"));
            fs::write(
                audit.join(env::var("NEXTEST_ATTEMPT_ID").unwrap()),
                path.to_str().unwrap(),
            )
            .unwrap();
            let first_attempt = env::var("NEXTEST_ATTEMPT").unwrap() == "1";
            let report = match mode.as_str() {
                "valid" => Some(r#"{"label":"wrapped","category":"wrapped"}"#.to_owned()),
                "label-only" => Some(r#"{"label":"wrapped"}"#.to_owned()),
                "absent" => None,
                "invalid" => Some("not json".to_owned()),
                "invalid-label" => Some(r#"{"label":"bad\nlabel"}"#.to_owned()),
                "invalid-category" => Some(r#"{"label":"wrapped","category":""}"#.to_owned()),
                "oversized" => Some("x".repeat(4097)),
                "directory" => {
                    fs::create_dir(&path).unwrap();
                    None
                }
                #[cfg(unix)]
                "fifo" => {
                    assert!(
                        std::process::Command::new("mkfifo")
                            .arg(&path)
                            .status()
                            .expect("create report FIFO")
                            .success()
                    );
                    None
                }
                "retry" if first_attempt => Some("not json".to_owned()),
                "retry" => Some(r#"{"label":"wrapped","category":"wrapped"}"#.to_owned()),
                "retry-absent" if first_attempt => {
                    code = 1;
                    Some(r#"{"label":"first attempt","category":"previous"}"#.to_owned())
                }
                "retry-absent" => None,
                _ => panic!("unknown wrapper report mode: {}", mode),
            };
            if let Some(report) = report {
                fs::write(path, report).unwrap();
            }
        }
    }
    std::process::exit(code);
}
