---
icon: material/broadcast
description: Design document describing how nextest performs signal handling.
---

# Signal handling

!!! abstract "Design document"

    This is a design document intended for nextest contributors and curious readers.

Nextest's signal handling uses Tokio's [native support for signals]. Signals are
received by a [multiplexer] called `SignalHandler`, which generates a stream of
events. The [runner loop's dispatcher] then selects over this stream. On receiving a signal, the
dispatcher is responsible for broadcasting a message to all units.

[native support for signals]: https://docs.rs/tokio/latest/tokio/signal/index.html
[multiplexer]: https://docs.rs/nextest-runner/latest/nextest_runner/signal/index.html
[runner loop's dispatcher]: runner-loop.md#dispatcher

Here's a simple sequence diagram of how signals are handled:

``` mermaid
sequenceDiagram
  autonumber
  participant tokio as Tokio
  participant muxer as signal muxer
  participant dispatcher as dispatcher
  muxer->>tokio: install signal handlers
  tokio->>+muxer: stream signals
  muxer->>-dispatcher: stream event
  activate dispatcher
  dispatcher->>+units: broadcast signal
  units-->>-dispatcher: respond if needed
  deactivate dispatcher
```

## Signal handling on Unix

Unix platforms have a rich set of signals, and nextest interacts with them in a
few different ways.

### Process groups

On Unix platforms, nextest creates a new [_process group_] for each unit of
work. Process groups do not form a tree; this directly impacts signal handling
in interactive use.

* For example, when the user presses Ctrl-C in a terminal, the terminal sends
  `SIGINT` to the foreground process group.
* If nextest did not create a new process group for each unit, the terminal would
  have sent `SIGINT` to both nextest and all unit processes.
* Since nextest creates a new process group for each unit, the terminal only
  sends `SIGINT` to nextest. It is then nextest's responsibility to send the signal
  to all child process groups.

For more about process groups, see Rain's RustConf 2023 talk: [_Beyond Ctrl-C:
the dark corners of Unix signal handling_][rustconf-talk]. There's an edited
transcript of the talk on [Rain's
blog](https://sunshowers.io/posts/beyond-ctrl-c-signals/). (Rain is nextest's
primary author and maintainer.)

[rustconf-talk]: https://www.youtube.com/watch?v=zhbkp_Fzqoo
[_process group_]: https://en.wikipedia.org/wiki/Process_group

### Shutdown signal handling

On receiving a signal like `SIGINT` or `SIGTERM`, the dispatcher sends a message
to all units to terminate with the same signal. Units then follow a flow similar
to timeouts, where they have a grace period to exit cleanly before being killed.

Currently, nextest sends `SIGINT` on receiving `SIGINT`, `SIGTERM` on receiving
`SIGTERM`, and so on. There is no _inherent_ reason the two have to be the same,
other than a general expectation of "abstraction coherence" (a kind of [least
astonishment]): if you set up process groups, you should behave similarly to the
world in which you don't set up process groups. This is a good principle to
follow, but it's not a hard requirement.

[least astonishment]: https://en.wikipedia.org/wiki/Principle_of_least_astonishment

### Job control { #job-control }

On Unix platforms, nextest supports [job control] via the `SIGTSTP` and `SIGCONT`
signals.

[job control]: https://en.wikipedia.org/wiki/Job_control_(Unix)

!!! info "`SIGTSTP` vs `SIGSTOP`"

    There are two closely-related signals in Unix: `SIGTSTP` and `SIGSTOP`. The
    main difference between the two is that `SIGTSTP` can be caught and handled
    by a process (as it is by nextest), while `SIGSTOP` cannot.

    When the user presses Ctrl-Z in a terminal, the signal that's sent is
    `SIGTSTP`, not `SIGSTOP`. We are lucky that is the case, because it gives
    nextest an opportunity to do some bookkeeping before stopping itself.

When a `SIGTSTP` is received, the dispatcher sends a message to all units to stop.
At that point, units do two things:

* **Send `SIGTSTP` to their associated process groups.** Since nextest creates
  a separate process group for each test, it must forward the signal to the
  process group of the test.
* **Pause all running timers.** This is very important! If the run is paused
  in the middle of a test being executed, the time spent in the pause window
  must not be included in the test's total run time. This is particularly important
  for timeouts imposed by nextest.

Similarly, when a `SIGCONT` is received, the dispatcher sends a message to all
units to resume. Units then send `SIGCONT` to their associated process groups,
and resume all paused timers.

### Pausable timers

Tokio itself doesn't have great support for pausing timers, other than a [global
pause](https://docs.rs/tokio/latest/tokio/time/fn.pause.html) in single-threaded
runtimes that's mostly geared towards `#[tokio::test]`. So nextest manages its
own pausable timers. This is done via two structs [inside the
`nextest_runner::time` module](https://github.com/nextest-rs/nextest/tree/main/nextest-runner/src/time):

* **`Stopwatch` to measure time.** A `Stopwatch` is a pair of `(start time, instant)` which also tracks a pause
  state, along with a duration accounting for previous pauses.
* **`PausableSleep` to wait until a duration has elapsed.**
  `PausableSleep` is a wrapper around a [Tokio sleep] which also tracks a pause
  state.

Each step in the executor is responsible for calling `pause` and `resume` on all
timers it has access to. Unfortunately, we haven't quite figured out an automatic
way of ensuring that everything that should be paused is paused, so this process
requires manual review.

Both of these types are useful in general, and could be extracted into a library
if there's interest.

[Tokio sleep]: https://docs.rs/tokio/latest/tokio/time/fn.sleep.html

### Double-spawning processes

On Unix platforms, when spawning a child process, nextest does not directly
spawn the child. Instead, it [spawns a copy of itself], which then spawns the
process using `exec`.

[spawns a copy of itself]: https://docs.rs/nextest-runner/latest/nextest_runner/double_spawn/index.html

(Note: This is done by calling [`posix_spawn`][posix_spawn] on the current
`cargo-nextest` executable with a hidden `__double-spawn` command, _not_
via the [`fork` system call][fork-syscall]. `fork` is quite messy at best
and actively dangerous at worst, and is best to be avoided.)

[fork-syscall]: https://en.wikipedia.org/wiki/Fork_(system_call)

This double-spawn approach works around a gnarly race with `SIGTSTP` handling.
If a child process receives `SIGTSTP` at exactly the wrong time (a window of
around 5ms on 2022-era hardware under load), it can get stuck in a "half-born"
paused state, and the parent process can get stuck in an uninterruptible sleep
state waiting for the child to finish spawning.

!!! info "This bug is universal"

    This race exists with _all_ [`posix_spawn`][posix_spawn] invocations on
    Linux, and likely on most other Unix platforms. It appears to be a flaw with
    the `posix_spawn` specification, and any implementations that don't
    specifically account for this issue are likely to have this bug.

    With nextest, users just hit it more often because nextest spawns a lot of
    processes very quickly.

[posix_spawn]: https://pubs.opengroup.org/onlinepubs/9699919799/functions/posix_spawn.html

You can reproduce the issue yourself by setting `NEXTEST_DOUBLE_SPAWN=0`, then
running `cargo nextest run -j64` against a repository; [the clap
repository] is a good candidate because it has many small tests. Now try hitting `Ctrl-Z` a few times, running
`fg` to resume nextest after each pause. You'll likely see one run in ten or so
where nextest gets stuck.

With the double-spawn approach:

* Nextest first uses a [signal mask] to temporarily block `SIGTSTP`.
* It then spawns a copy of itself, which inherits the signal mask.
* At this point, the spawn is complete, and the parent process can carry on.
* The child copy then unblocks `SIGTSTP`.

  A queued up `SIGTSTP` may be received at this point. If that is so, the process
  is paused. But, importantly, the parent does not get stuck waiting for the child to finish spawning.

* Finally, the spawned child uses [`Command::exec`](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.exec)
  to replace itself with the test or script process.

The double-spawn approach completely addresses this race, and no instances of a
stuck runner have been observed since it was implemented.

[the clap repository]: https://github.com/clap-rs/clap
[signal mask]: https://www.gnu.org/software/libc/manual/html_node/Process-Signal-Mask.html

## Signal handling on Windows { #on-windows }

Windows has a much simpler signal model than Unix.

* For console applications, Windows supports pressing Ctrl-C, which resembles `SIGINT`.
* Windows also has the notion of [console process groups]. However, the way in which
  they work is quite different from Unix process groups. For example, Ctrl-C events
  [cannot be limited] to a single process group.
* For graphical applications, Windows supports [the `WM_CLOSE` message](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-close).

In many ways, the `WM_CLOSE` message is more reasonable than Unix signals,
because it's a message that can be handled or ignored, and ties nicely into an
event-driven model such as that of nextest. It is geared towards GUI
applications, but can be used in console applications as well by creating a
hidden window. See [this StackOverflow
post](https://stackoverflow.com/questions/8698881/intercept-wm-close-for-cleanup-operations)
for more.

Currently, nextest does not put tests into separate process groups on Windows,
nor does it send `WM_CLOSE` messages. This is an area where nextest could
benefit from Windows expertise.

[console process groups]: https://docs.microsoft.com/en-us/windows/console/console-process-groups
[cannot be limited]: https://learn.microsoft.com/en-us/windows/console/generateconsolectrlevent

### Job objects

On Windows, nextest uses [job objects] to manage the lifetime of all child
processes. Job objects don't support graceful termination like Unix signals do.
The only method available is [`TerminateJobObject`][terminate-job-object], which
is equivalent to a Unix `SIGKILL`.

Unlike process groups, job objects form a tree. If something else runs nextest
within a job object and then calls `TerminateJobObject`, both nextest and all
its child processes are terminated.

When a test times out, nextest calls `TerminateJobObject` on the job object
associated with the test immediately. In the future, it would be interesting
to send a Ctrl-C (or maybe a `WM_CLOSE`?) to the test process first.

When nextest receives a Ctrl-C, it assumes that child tests will also receive
the same Ctrl-C and terminate themselves. If tests don't exit within the grace
period (by default, 10 seconds), nextest will terminate them via their
corresponding job object.

[job objects]: https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
[terminate-job-object]: https://docs.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-terminatejobobject

_Last substantive revision: 2024-12-17_
