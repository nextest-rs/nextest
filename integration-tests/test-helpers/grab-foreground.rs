// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A test helper that grabs foreground process group access.
//!
//! This simulates what interactive shells do when they start up: they open
//! `/dev/tty` directly (bypassing any stdin redirection) and call `tcsetpgrp`
//! to become the foreground process group. This causes the parent process to
//! become a background process, which will receive SIGTTOU if it tries to
//! modify terminal state.
//!
//! See <https://github.com/nextest-rs/nextest/issues/2878>.

fn main() {
    #[cfg(unix)]
    unix::grab_foreground();
}

#[cfg(unix)]
mod unix {
    use std::{fs::File, os::fd::AsRawFd};

    pub(super) fn grab_foreground() {
        // Open /dev/tty directly, like interactive shells do. This bypasses
        // any stdin redirection that nextest might have set up.
        eprintln!("[grab-foreground] opening /dev/tty");
        let tty = match File::open("/dev/tty") {
            Ok(f) => f,
            Err(e) => {
                // No controlling terminal (e.g., in CI). This is expected.
                eprintln!("[grab-foreground] failed to open /dev/tty (expected in CI): {e}");
                return;
            }
        };
        let tty_fd = tty.as_raw_fd();
        eprintln!("[grab-foreground] opened /dev/tty");

        // Ignore SIGTTOU before calling tcsetpgrp. This is what zsh does:
        // https://github.com/zsh-users/zsh/blob/3e72a52/Src/init.c#L1439
        // Without this, tcsetpgrp from a background process would cause us to
        // receive SIGTTOU and stop. Ignoring SIGTTOU has the special effect of
        // allowing tcsetpgrp to succeed even from a background process.
        //
        // See also: https://github.com/zsh-users/zsh/blob/3e72a52/Src/exec.c#L1134-L1143
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        }

        // Create a new process group with this process as the leader.
        let pid = unsafe { libc::getpid() };
        let res = unsafe { libc::setpgid(pid, pid) };
        if res == -1 {
            eprintln!(
                "[grab-foreground] setpgid failed: {}",
                std::io::Error::last_os_error()
            );
            std::process::exit(1);
        }
        eprintln!("[grab-foreground] created new process group: {pid}");

        // Grab the foreground process group on the controlling terminal.
        let res = unsafe { libc::tcsetpgrp(tty_fd, pid) };
        if res == -1 {
            eprintln!(
                "[grab-foreground] tcsetpgrp failed: {}",
                std::io::Error::last_os_error()
            );
            std::process::exit(1);
        }

        eprintln!("[grab-foreground] successfully grabbed foreground");
    }
}
