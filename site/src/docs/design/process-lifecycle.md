---
icon: material/state-machine
sidebar_icon: false
description: Design document describing the lifecycle of a test process.
---

# Process lifecycle

!!! abstract "Design document"

    This is a design document intended for nextest contributors and curious readers.

    For why nextest spawns a process for each test, see [_Why process-per-test?_](why-process-per-test.md)

This document describes the mechanism by which nextest spawns processes efficiently and robustly. It is structured in two parts: a quick summary, and a detailed narrative through a series of approaches of increasing complexity. 

## Summary

Nextest's basic mission as a test runner is to handle all the ways in which tests go wrong. Within the context of test process lifecycles, nextest assumes that _test processes can behave in arbitrarily bad ways_. This results in the process lifecycle having a high degree of essential complexity, but a constrained design space with few degrees of freedom.

We develop nextest's process lifecycle algorithm through a succession of problems and their solutions. The first few sections are relatively well-known problems — if you're already familiar with a particular problem, feel free to skip that section.

## More details

Much of what nextest does is spawn and manage test processes. Spawning a child process and reading its output seems quite simple at the surface:

```rust
use std::process::Command;

let output = Command::new("my-process")
    .arg("my-arg")
    .output()?;

println!("status: {}", output.status);
print!("stdout: {}", String::from_utf8_lossy(&output.stdout));
eprint!("stderr: {}", String::from_utf8_lossy(&output.stderr));
```

The [`Command::output`][Command::output] method performs a sequence of steps:

1. Create two [anonymous pipes](https://en.wikipedia.org/wiki/Anonymous_pipe), one for standard output and one for standard error.
2. Spawn the child process, setting the standard output and standard error to the write end of each pipe respectively.
3. _Simultaneously_ read from the standard output and standard error pipes, continuing until the write ends of both pipes are closed.
4. Wait until the child process exits.

The _simultaneously_ in step 3 is key, and avoids a common issue in simpler implementations of waiting for child processes.

### The simplest (incorrect) implementation

!!! tip "Skip ahead if…"

    If you're already familiar with pipe deadlocks, skip ahead to [_Grandchild processes and leaky tests_](#grandchild-processes-and-leaky-tests). A summary of this section: if stdout and stderr are read sequentially, they can deadlock, so nextest reads both simultaneously.

Consider this attempted manual implementation of [`Command::output`][Command::output]:

```rust
use std::{
    io::{self, Read},
    process::{Command, Output, Stdio},
};

fn command_output(command: &mut Command) -> io::Result<Output> {
    // Spawn the child process.
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Read standard output to the end.
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout was configured as a pipe")
        .read_to_end(&mut stdout)?;

    // Read standard error to the end.
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("stderr was configured as a pipe")
        .read_to_end(&mut stderr)?;

    // Wait for the child process to exit.
    let status = child.wait()?;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}
```

For the vast majority of tests, this implementation is likely fine. But pipes, being asynchronous, buffer data internally. The size of this buffer is not unbounded; rather:

* On Unix platforms, the maximum pipe size varies by platform. A typical maximum is 65 536 bytes ([reference](https://unix.stackexchange.com/a/11954)).
* Windows is more flexible, but Rust [requests](https://github.com/rust-lang/rust/blob/feaadeeaca7db0594da854e7c8c07495341c7439/library/std/src/sys/process/windows/child_pipe.rs#L58-L59) a buffer capacity of 65 536 bytes to match typical Unix values.

But what if a process writes more than 65 536 bytes? Consider this program:

```rust
use std::io::{self, Write};

fn main() -> io::Result<()> {
    // Write more data than the standard error pipe can typically hold.
    let data = vec![b'x'; 1024 * 1024];
    io::stderr().write_all(&data)?;

    println!("done");
    Ok(())
}
```

The parent process will attempt to read standard output until it is closed. Meanwhile, the child process blocks until the parent process reads almost all of its standard error. This is a textbook example of a _pipe deadlock_ — this specific deadlock has a long and tortured history.

You might imagine being able to read data from each pipe, sequentially, in chunks. But for any such strategy it is possible to construct a program which can defeat it. So the only option when dealing with arbitrary child processes is to read from both, _simultaneously_.

How can we read from two pipes simultaneously? In a typical straight-line program, we cannot. There are a few ways to do it: one is to create a separate thread for reading from each pipe; another is to switch to an asynchronous, event-driven model. The latter is what the Rust standard library uses ([via `poll` on Unix](https://github.com/rust-lang/rust/blob/feaadeeaca7db0594da854e7c8c07495341c7439/library/std/src/sys/process/unix/common.rs#L640), [and `WaitForMultipleObjects` on Windows](https://github.com/rust-lang/rust/blob/feaadeeaca7db0594da854e7c8c07495341c7439/library/std/src/sys/process/windows/child_pipe.rs#L422)), and (through a somewhat different approach) what nextest uses as well.

### Grandchild processes and leaky tests

!!! tip "Skip ahead if…"

    * If you're already familiar with how inherited handles keep pipes open, skip ahead to [_Dealing with grandchild leaks_](#dealing-with-grandchild-leaks) for the solution.
    * If you're already familiar with how async Rust can handle grandchild leaks, skip ahead to [_Handling signals as events_](#handling-signals-as-events).

    A summary of this section: because grandchild processes can keep file handles open past the end of a child process, nextest must wait for the outputs to close and the process to exit simultaneously; doing this in a composable manner is best achieved through async Rust.

Next, let's look at the second half of step 3: waiting until the write ends of the pipes are closed. This is a surprisingly tricky problem, and is the simplest example of the Rust standard library's approach being insufficient.

One of the properties of a file descriptor (Unix) or a file handle (Windows), such as one end of a pipe, is that it can be _duplicated_.

* In particular, an easy way to duplicate standard output or standard error pipes is to let child processes inherit them. (This is what [`Command::spawn`][Command::spawn] does by default, for instance.)
* It is common for tests, especially the larger-scale integration tests nextest is designed for, to spawn child processes. Often those processes inherit standard output and standard error.
* One end of a pipe is not considered closed until _all_ duplicated file descriptors/handles to it are closed.

This leads to cases like the following innocuous-looking child process leaking standard output and standard error:

```rust
use std::{io, process::Command};

fn main() -> io::Result<()> {
    // Spawn a child process which inherits standard output and standard error,
    // and sleeps for 30 seconds.
    Command::new("sleep")
        .arg("30")
        .spawn()?;

    println!("parent exiting");
    Ok(())
}
```

`main()`, and therefore the process, exits quickly. But consider what happens with the `sleep` _grandchild_ process, which inherits standard output and standard error, and only exits 30 seconds later. Because [`Command::output`][Command::output] waits for standard output and standard error to close before waiting for the child process to exist, calling it against this program will indeed hang for 30 seconds.

For a test runner, this behavior is quite bad! In particular, failing tests are more likely to leak processes, which can turn into stuck test runs and a frustrating experience. A failing test means the user is already having a bad day — it is important that the test runner not make their day worse.

!!! note "What about cargo test?"

    Does `cargo test` avoid this issue? Sort of, because it is a much simpler test runner and doesn't create pipes for test processes, instead letting them inherit standard output and standard error. But that just means the problem is moved one level up — if whatever is supervising `cargo test` is not resilient to grandchild leaks, the same issue recurs.

#### Dealing with grandchild leaks

How should a test runner deal with grandchild leaks? Just like in the previous section, the issue is that an algorithm is sequential when it ought to be simultaneous. In the implementation of [`Command::output`](Command::output), step 3, waiting on the file handles to close, comes _sequentially_ before step 4, waiting for the process to exit. Nextest instead performs steps 3 and 4 _concurrently_: it reads from output and waits for the process to exit at the same time. If a process exits but the corresponding handles are not closed within a short while after that, the test is marked as [leaky](https://nexte.st/docs/features/leaky-tests/).

Unlike pipe deadlocks, which can be solved by simultaneously waiting across the same kind of event source (i.e., two file descriptors), in this case the simultaneous wait is across _different_ kinds of event sources (two file descriptors and a process exit). We call this pattern a _heterogenous select_. Nextest uses async Rust and [Tokio](https://tokio.rs/) to perform heterogenous selects; for more information, see [this blog post](https://sunshowers.io/posts/nextest-and-tokio/).

!!! info "Async Rust is composable"

    An important detail is that even though [`Command::output`][Command::output] does a simultaneous wait internally, it is not composable with other sources of asynchronicity; you cannot say "continue with what `Command::output` is doing internally, but also wait for the process to exit concurrently". This kind of composition is the raison d'être of async Rust. For [all its](https://sunshowers.io/posts/cancelling-async-rust) [faults](http://rfd.shared.oxide.computer/rfd/0609), nextest must use async Rust to achieve this degree of rigor.

### Handling signals as events

Using async Rust allows us to handle many other kinds of event sources, such as signals on Unix or Ctrl-C on Windows. Tokio allows signals to be handled as events, and nextest simultaneously waits on these events as well. (One could think of a nextest run as spinning up a little in-process server for every test.)

For more information, see [_Signal handling_](architecture/signal-handling.md). Summarizing that document: nextest creates a process group for each test, taking responsibility for forwarding signals when it receives any.

### `fork`/`exec` and `posix_spawn`

!!! tip "Skip ahead if…"

    If you're already familiar with `fork`/`exec` versus `posix_spawn` [_`fork/exec` and `posix_spawn`_](#grandchild-processes-and-leaky-tests). A summary of this section: on Unix, because of the inefficiency and complexity of `fork`/`exec`, nextest tries quite hard to ensure the Rust standard library uses `posix_spawn` over it.

Now we turn our attention to step 2: spawning the child process with [`Command::spawn`]. On Unix, the classic way to spawn processes is with a pair of function calls, `fork` and `exec`.

* The `fork` function ([reference](https://pubs.opengroup.org/onlinepubs/9799919799/functions/fork.html)) creates a new process with the same memory contents as the current process.
* The `exec` family of functions ([reference](https://pubs.opengroup.org/onlinepubs/9799919799/functions/exec.html)) replaces the contents of the current process with a new one.

`fork`/`exec` is elegant in its orthogonality, and flexible: you can run any [async-signal-safe code](https://man7.org/linux/man-pages/man7/signal-safety.7.html) within it. But:

* `fork`/`exec` is quite inefficient: even with copy-on-write memory, the kernel needs to do a lot of bookkeeping.
* `fork`/`exec` is also very difficult to use correctly:
  * In multithreaded programs, what happens to the other threads in the fork? (For the answer, see the [reference](https://pubs.opengroup.org/onlinepubs/9799919799/functions/fork.html).)
  * How can errors in the space between `fork` and `exec` be communicated to the parent process? (Via a pipe — something that will become relevant later in TKTK).
  * TKTK mention fork handlers?
* Most programs don't need the full flexibility of `fork`/`exec` — they're interested in doing a few basic things like creating a process group and setting the current directory.

Because of this, modern POSIX systems provide an alternative called `posix_spawn` ([reference](https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_spawn.html)) instead. `posix_spawn` provides a more limited, declarative interface. For example:

* [`posix_spawnattr_setpgroup`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_spawnattr_getpgroup.html) sets or creates a process group.
* [`posix_spawn_file_actions_addchdir`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_spawn_file_actions_addchdir.html) sets the current directory of the child process.

In return, `posix_spawn` provides an easy-to-use interface that's also faster. These days, if you're writing C, you're probably reaching for `posix_spawn`.

In the Rust standard library, these differences are abstracted away behind [`Command::spawn`][Command::spawn]. Rust uses `posix_spawn` if it is available, behaves correctly, and supports the requested options, and `fork`/`exec` otherwise. With modern versions of Rust:

* With a simple [`Command::spawn`](Command::spawn) without options, Rust uses `posix_spawn` on Linux, macOS, FreeBSD, illumos, and a few other platforms. (Why not all POSIX platforms? Because [`Command::spawn`][Command::spawn] is guaranteed to return an `ENOENT` error if the executable isn't found, but the standard allows for the binary to [exit with code 127](https://pubs.opengroup.org/onlinepubs/9799919799/functions/posix_spawn.html) instead. The allowlisted platforms all return an `ENOENT` error.)
* Support for changing the current directory is a relatively new addition, and older installations might not support it. So if [`Command::current_dir`][Command::current_dir] is called and Rust isn't sure about this, it performs runtime detection to see if the corresponding `posix_spawn_file_actions_addchdir` or its `_np` version is available.
* If any kind of [`pre_exec` hook](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.pre_exec) is configured, [`Command::spawn`][Command::spawn] always falls back to `fork`/`exec`.

Some of the things that follow from this:

1. The rules are quite complicated! TODO complete

### Double-spawning processes

TKTK

### Process spawn synchronization

TKTK

[Command::output]: https://doc.rust-lang.org/std/process/struct.Command.html#method.output
[Command::spawn]: https://doc.rust-lang.org/std/process/struct.Command.html#method.spawn
