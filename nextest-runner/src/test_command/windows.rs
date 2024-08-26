// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{
    io,
    os::windows::{ffi::OsStrExt as _, io::FromRawHandle as _, prelude::OwnedHandle},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation as fnd, Security::SECURITY_ATTRIBUTES, Storage::FileSystem as fs,
    System::Pipes as pipe,
};

pub struct State {
    pub(super) ours: OwnedHandle,
}

pub(super) fn setup_io(cmd: &mut std::process::Command) -> io::Result<State> {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    static RANDOM_SEQ: once_cell::sync::OnceCell<AtomicUsize> = once_cell::sync::OnceCell::new();
    let rand_seq = RANDOM_SEQ.get_or_init(|| {
        use rand::{rngs::OsRng, RngCore};
        AtomicUsize::new(OsRng.next_u32() as _)
    });

    // A 64kb pipe capacity is the same as a typical Linux default.
    const PIPE_BUFFER_CAPACITY: u32 = 64 * 1024;
    const FLAGS: u32 =
        fs::FILE_FLAG_FIRST_PIPE_INSTANCE | fs::FILE_FLAG_OVERLAPPED | fs::PIPE_ACCESS_INBOUND;

    unsafe {
        let ours;
        let mut wide_path;
        let mut tries = 0;
        let mut reject_remote_clients_flag = pipe::PIPE_REJECT_REMOTE_CLIENTS;
        loop {
            tries += 1;
            let name = format!(
                r"\\.\pipe\__nextest_pipe__.{}.{}",
                std::process::id(),
                rand_seq.fetch_add(1, SeqCst),
            );
            wide_path = std::ffi::OsStr::new(&name)
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();

            let handle = pipe::CreateNamedPipeW(
                wide_path.as_ptr(),
                FLAGS,
                pipe::PIPE_TYPE_BYTE
                    | pipe::PIPE_READMODE_BYTE
                    | pipe::PIPE_WAIT
                    | reject_remote_clients_flag,
                1,
                PIPE_BUFFER_CAPACITY,
                PIPE_BUFFER_CAPACITY,
                0,
                null_mut(),
            );

            // We pass the `FILE_FLAG_FIRST_PIPE_INSTANCE` flag above, and we're
            // also just doing a best effort at selecting a unique name. If
            // `ERROR_ACCESS_DENIED` is returned then it could mean that we
            // accidentally conflicted with an already existing pipe, so we try
            // again.
            //
            // Don't try again too much though as this could also perhaps be a
            // legit error.
            // If `ERROR_INVALID_PARAMETER` is returned, this probably means we're
            // running on pre-Vista version where `PIPE_REJECT_REMOTE_CLIENTS` is
            // not supported, so we continue retrying without it. This implies
            // reduced security on Windows versions older than Vista by allowing
            // connections to this pipe from remote machines.
            // Proper fix would increase the number of FFI imports and introduce
            // significant amount of Windows XP specific code with no clean
            // testing strategy
            // For more info, see https://github.com/rust-lang/rust/pull/37677.
            if handle == fnd::INVALID_HANDLE_VALUE {
                let err = io::Error::last_os_error();
                let raw_os_err = err.raw_os_error();
                if tries < 10 {
                    if raw_os_err == Some(fnd::ERROR_ACCESS_DENIED as i32) {
                        continue;
                    } else if reject_remote_clients_flag != 0
                        && raw_os_err == Some(fnd::ERROR_INVALID_PARAMETER as i32)
                    {
                        reject_remote_clients_flag = 0;
                        tries -= 1;
                        continue;
                    }
                }
                return Err(err);
            }
            ours = OwnedHandle::from_raw_handle(handle as _);
            break;
        }

        // Note we differ from rust here because it sets the SECURITY_ATTRIBUTES
        // but that method is not exposed on OpenOptionsExt so we need to do the
        // call manually

        // Open the write end of the file that we'll pass to the child process
        let handle = fs::CreateFileW(
            wide_path.as_ptr(),
            fnd::GENERIC_WRITE,
            0,
            &SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as _,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            },
            fs::OPEN_EXISTING,
            0,
            null_mut(),
        );

        if handle == fnd::INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let handle = OwnedHandle::from_raw_handle(handle as _);

        // Use the handle for stdout AND stderr
        cmd.stdout(handle.try_clone()?).stderr(handle.try_clone()?);

        Ok(State { ours })
    }
}
