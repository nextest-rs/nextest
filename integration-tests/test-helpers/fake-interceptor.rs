// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A fake interceptor binary for testing debugger and tracer integration.
//!
//! This simulates what real interceptors (debuggers/tracers) do:
//! - Takes program and args
//! - Prints diagnostic info to stderr
//! - Execs the actual test binary
//!
//! Unlike real interceptors, it doesn't actually debug/trace - it just validates
//! that nextest set up the environment correctly for the mode.
//!
//! Modes:
//! - debugger: Simulates GDB/LLDB behavior
//! - tracer: Simulates strace/ltrace behavior

use std::{
    env, fmt,
    io::{self, IsTerminal},
    process::{Command, exit},
};

#[derive(Debug, PartialEq)]
enum InterceptorMode {
    Debugger,
    Tracer,
}

impl fmt::Display for InterceptorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InterceptorMode::Debugger => write!(f, "debugger"),
            InterceptorMode::Tracer => write!(f, "tracer"),
        }
    }
}

/// Verify stdin behavior based on interceptor mode.
/// - Debugger: stdin should be readable (passthrough enabled)
/// - Tracer: stdin should be null (passthrough disabled)
fn verify_stdin(mode: &InterceptorMode) {
    let stdin = io::stdin();
    let is_terminal = stdin.is_terminal();

    // Try to check if stdin is /dev/null on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = stdin.as_raw_fd();

        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let fstat_result = unsafe { libc::fstat(fd, &mut stat) };

        if fstat_result == 0 {
            // Check if it's a character device (like /dev/null).
            let is_char_device = (stat.st_mode & libc::S_IFMT) == libc::S_IFCHR;

            // /dev/null has specific major/minor numbers, but we'll just check
            // if it's seekable. A real TTY or pipe won't be seekable, but
            // /dev/null is.
            let current_pos = unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) };

            match mode {
                InterceptorMode::Debugger => {
                    // Debuggers should have stdin passthrough (TTY or pipe, not /dev/null).
                    eprintln!(
                        "[fake-{mode}] stdin check: is_terminal={}, is_char_device={}, seekable={}",
                        is_terminal,
                        is_char_device,
                        current_pos >= 0
                    );
                    if is_char_device && current_pos >= 0 && !is_terminal {
                        // This looks like /dev/null: might be wrong for
                        // debuggers, though we can't be sure because the parent
                        // process might have redirected stdin to /dev/null.
                        eprintln!(
                            "[fake-{mode}] WARNING: stdin appears to be /dev/null \
                             (debugger expects passthrough)",
                        );
                    }
                }
                InterceptorMode::Tracer => {
                    // Tracers should always have stdin set to /dev/null.
                    eprintln!(
                        "[fake-{mode}] stdin check: is_terminal={}, is_char_device={}, seekable={}",
                        is_terminal,
                        is_char_device,
                        current_pos >= 0
                    );
                    if is_char_device && current_pos >= 0 {
                        eprintln!("[fake-{mode}] stdin is /dev/null (expected for tracer)");
                    } else {
                        panic!("[fake-{mode}] tracer stdin is not /dev/null");
                    }
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        eprintln!("[fake-{mode}] stdin check: is_terminal={}", is_terminal);
    }
}

/// Verify process group behavior based on interceptor mode.
///
/// - Debugger: should NOT be in separate process group
/// - Tracer: SHOULD be in separate process group
#[cfg(unix)]
fn verify_process_group(mode: &InterceptorMode) {
    let pid = unsafe { libc::getpid() };
    let pgid = unsafe { libc::getpgid(0) };
    let parent_pid = unsafe { libc::getppid() };
    let parent_pgid = unsafe { libc::getpgid(parent_pid) };

    eprintln!(
        "[fake-{mode}] process group: pid={}, pgid={}, parent_pid={}, parent_pgid={}",
        pid, pgid, parent_pid, parent_pgid
    );

    let in_own_process_group = pgid == pid;
    let in_parent_process_group = pgid == parent_pgid;

    match mode {
        InterceptorMode::Debugger => {
            // Debugger should NOT create a new process group.
            if in_own_process_group && !in_parent_process_group {
                panic!(
                    "[fake-{mode}]: in own process group \
                     (debugger should not create a process group)",
                );
            } else {
                eprintln!("[fake-{mode}] process group check: ok (not in separate process group)");
            }
        }
        InterceptorMode::Tracer => {
            // Tracer SHOULD create a new process group.
            if in_own_process_group {
                eprintln!("[fake-{mode}] process group check: ok (in own process group)",);
            } else {
                panic!(
                    "[fake-{mode}] not in own process group \
                     (tracer should create a process group)",
                );
            }
        }
    }
}

#[cfg(not(unix))]
fn verify_process_group(mode: &InterceptorMode) {
    eprintln!("[fake-{mode}] process group check: skipped (not Unix)");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Determine mode from first arg - must be --mode=debugger or --mode=tracer
    let mode = if args.len() > 1 && args[1] == "--mode=debugger" {
        InterceptorMode::Debugger
    } else if args.len() > 1 && args[1] == "--mode=tracer" {
        InterceptorMode::Tracer
    } else {
        eprintln!(
            "[fake-interceptor] ERROR: first argument must be --mode=debugger or --mode=tracer"
        );
        eprintln!(
            "[fake-interceptor] usage: fake-interceptor --mode=<debugger|tracer> <program> [program-args]"
        );
        exit(99);
    };

    eprintln!("[fake-interceptor] mode: {}", mode);

    // Verify mode-specific properties.
    verify_stdin(&mode);
    verify_process_group(&mode);

    // The program to run is at index 2 (after binary name and --mode flag).
    if args.len() < 3 {
        eprintln!("[fake-{mode}] ERROR: no program found after mode flag");
        exit(101);
    }

    let program = &args[2];
    let program_args = &args[3..];

    eprintln!("[fake-{mode}] program: {}", program);
    eprintln!("[fake-{mode}] program args: {:?}", program_args);

    // Check for the NEXTEST_EXECUTION_MODE environment variable to ensure that
    // we're in a nextest context.
    assert_eq!(
        std::env::var("NEXTEST_EXECUTION_MODE").as_deref(),
        Ok("process-per-test"),
        "NEXTEST_EXECUTION_MODE set to process-per-test"
    );

    // Run the test.
    eprintln!("[fake-{mode}] execing: {} {:?}", program, program_args);
    match Command::new(program).args(program_args).status() {
        Ok(status) => match status.code() {
            Some(code) => {
                eprintln!("[fake-{mode}] program exited with code: {}", code);
                exit(code);
            }
            None => {
                eprintln!("[fake-{mode}] program terminated by signal");
                exit(102);
            }
        },
        Err(err) => {
            eprintln!(
                "[fake-{mode}] failed to spawn program '{}': {}",
                program, err
            );
            exit(103);
        }
    }
}
