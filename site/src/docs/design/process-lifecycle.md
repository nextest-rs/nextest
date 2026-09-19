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

TKTK

## More details

We cover a sequence of increasing levels of depth (TKTK reword)

### Introduction

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

The [`Command::output`] method performs a sequence of steps:

1. Create two [anonymous pipes](https://en.wikipedia.org/wiki/Anonymous_pipe), one for standard output and one for standard error.
2. Spawn the child process, setting the standard output and standard error to the write end of each pipe respectively.
3. _Simultaneously_ read from the standard output and standard error pipes, continuing until the write ends of both pipes are closed.
4. Wait until the child process exits.

The _simultaneously_ in step 3 is key, and avoids a common issue in simpler implementations of waiting for child processes.

### The simplest (incorrect) implementation

Consider this attempted manual implementation of [`Command::output`]:

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

Next, let's look at the second half of step 3: waiting until the write ends of the pipes are closed. This is a surprisingly tricky problem, and is the simplest example of the Rust standard library's approach being insufficient.

One of the properties of a file descriptor (Unix) or a file handle (Windows), such as one end of a pipe, is that it can be _duplicated_.

* In particular, an easy way to duplicate standard output or standard error pipes is to let child processes inherit them. (This is what [`Command::spawn`] does, for instance.)
* It is common for tests, especially the larger-scale integration tests nextest is designed for, to spawn child processes, and often those processes inherit standard output and standard error.
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

`main()`, and therefore the process, exits quickly. But consider what happens with the `sleep` _grandchild_ process, which inherits standard output and standard error, and only exits 30 seconds later. Because [`Command::output`] waits for standard output and standard error to close before waiting for the child process to exist, calling it against this program will indeed hang for 30 seconds.

For a test runner, this behavior is quite bad! In particular, failing tests are more likely to leak processes, which can turn into stuck test runs and a frustrating experience. A failing test means the user is already having a bad day; it is important that the test runner not make their day worse.

!!! note "What about cargo test?"

    Does `cargo test` avoid this issue? Sort of, because it is a much simpler test runner and doesn't create pipes for test processes, instead letting them inherit standard output and standard error. But the problem is just moved one level up — if whatever is supervising `cargo test` is not resilient to grandchild leaks, that can run into the same issue.

How should a test runner deal with this situation? Just like in the previous section, the issue is that an algorithm is sequential when it ought to be simultaneous. In this case, step 3, waiting on the file handles to close, comes sequentially before step 4, waiting for the process to exit. Nextest instead performs steps 3 and 4 simultaneously: it reads from output and waits for the process to exit at the same time. If a process exits but the corresponding handles are not closed within a short while after that, the test is marked as [leaky](https://nexte.st/docs/features/leaky-tests/).

Simultaneously waiting on process exit can be done either by piling more threads on top, or through more asynchronous, event-driven programming. An important detail is that [`Command::output`], while being async internally, is not composable with other sources of asynchronicity; you cannot easily say "continue with what an opaque function call is doing internally, but also wait for the process to exit at the same time". This kind of composition is the raison d'être of async Rust, and it forms the core of [why nextest uses Tokio](https://sunshowers.io/posts/nextest-and-tokio/).

### Signal handling

A related class of issues is around handling signals on Unix and Ctrl-C on Windows. The aforementioned Tokio backend allows for nextest to be entirely event-driven internally using the [actor model](https://en.wikipedia.org/wiki/Actor_model). (One could think of a nextest process as hosting lots of little in-process services, one per test.) In this model, signals become events, relatively unexceptional and processed the same as other events.

For more information on how nextest handles and forwards signals to child processes, see [the corresponding design document](architecture/signal-handling.md).

### `fork`/`exec` and `posix_spawn`

TKTK

### Process spawn synchronization

### Double-spawning processes

TKTK

[`Command::output`]: https://doc.rust-lang.org/std/process/struct.Command.html#method.output
[`Command::spawn`]: https://doc.rust-lang.org/std/process/struct.Command.html#method.spawn
