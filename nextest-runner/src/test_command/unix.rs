use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

use super::Stdio;

pub(super) struct State {
    ours: OwnedFd,
    #[allow(dead_code)]
    theirs: OwnedFd,
}

pub(super) fn setup_io(cmd: &mut std::process::Command) -> io::Result<State> {
    let mut fds = [0; 2];

    #[inline]
    fn cvt(res: libc::c_int) -> io::Result<libc::c_int> {
        if res == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }

    // This is copied from the std library
    // https://github.com/rust-lang/rust/blob/3095d31a759e569a9da3fe908541f301a211ea66/library/std/src/sys/unix/fd.rs#L413-L447
    cfg_if::cfg_if! {
        if #[cfg(not(any(
            target_env = "newlib",
            target_os = "solaris",
            target_os = "illumos",
            target_os = "emscripten",
            target_os = "fuchsia",
            target_os = "l4re",
            target_os = "linux",
            target_os = "haiku",
            target_os = "redox",
            target_os = "vxworks",
            target_os = "nto",
        )))] {
            #[inline]
            unsafe fn set_cloexec(fd: libc::c_int) -> io::Result<()> {
                cvt(libc::ioctl(fd, libc::FIOCLEX))?;
                Ok(())
            }
        } else if #[cfg(any(
            all(
                target_env = "newlib",
                not(any(target_os = "espidf", target_os = "horizon", target_os = "vita"))
            ),
            target_os = "solaris",
            target_os = "illumos",
            target_os = "emscripten",
            target_os = "fuchsia",
            target_os = "l4re",
            target_os = "haiku",
            target_os = "vxworks",
            target_os = "nto",
        ))] {
            #[inline]
            unsafe fn set_cloexec(fd: libc::c_int) -> io::Result<()> {
                let previous = cvt(libc::fcntl(fd, libc::F_GETFD))?;
                let new = previous | libc::FD_CLOEXEC;
                if new != previous {
                    cvt(libc::fcntl(fd, libc::F_SETFD, new))?;
                }
                Ok(())
            }
        } else {
            // Note we don't use compile_error! here since we don't provide implementations
            // for eg. linux since it's not ever called there, but if you get a compile
            // error due to set_cloexec not being defined, it's most likely you are trying
            // to compile for a target that doesn't support process spawning anyways
        }
    }

    // This is copied from the std library
    // https://github.com/rust-lang/rust/blob/3095d31a759e569a9da3fe908541f301a211ea66/library/std/src/sys/unix/pipe.rs#L14-L46
    let ours;
    let theirs;

    unsafe {
        // The only known way right now to create atomically set the CLOEXEC flag is
        // to use the `pipe2` syscall. This was added to Linux in 2.6.27, glibc 2.9
        // and musl 0.9.3, and some other targets also have it.
        cfg_if::cfg_if! {
            if #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "hurd",
                target_os = "linux",
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "redox"
            ))] {
                cvt(libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC))?;
                ours = std::os::fd::OwnedFd::from_raw_fd(fds[0]);
                theirs = std::os::fd::OwnedFd::from_raw_fd(fds[1]);
            } else {
                cvt(libc::pipe(fds.as_mut_ptr()))?;

                ours = std::os::fd::OwnedFd::from_raw_fd(fds[0]);
                theirs = std::os::fd::OwnedFd::from_raw_fd(fds[1]);

                set_cloexec(ours.as_raw_fd())?;
                set_cloexec(theirs.as_raw_fd())?;
            }
        }

        cmd.stderr(Stdio::from_raw_fd(theirs.as_raw_fd()))
            .stdout(Stdio::from_raw_fd(theirs.as_raw_fd()));
    }

    Ok(State { ours, theirs })
}

/// Immensely irritating, ChildStdout and ChildStderr are different types despite
/// being identical internally :p (to be fair, this problem is in std as well)
#[inline]
pub(super) fn stderr_to_stdout(stderr: tokio::process::ChildStderr) -> io::Result<super::Pipe> {
    super::Pipe::from_std(stderr.into_owned_fd()?.into())
}

#[inline]
pub(super) fn state_to_stdout(state: State) -> io::Result<super::Pipe> {
    super::Pipe::from_std(std::process::ChildStdout::from(state.ours))
}
