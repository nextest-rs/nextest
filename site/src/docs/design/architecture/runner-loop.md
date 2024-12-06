---
icon: material/run-fast
description: Nextest's runner loop is responsible for executing units of work, and coordinating scheduling and responses.
---

# The runner loop

Nextest's [runner loop] is responsible for executing _units_ of work, and
coordinating scheduling and responses. A unit of work is typically a single
attempt of a test, but can also be a setup (and in the future, teardown) script.

[runner loop]: https://docs.rs/nextest-runner/latest/nextest_runner/runner/index.html

## Background

Nextest's runner loop is one of the oldest parts of the codebase, and has
evolved over time from being synchronous Rust to using Tokio's asynchronous
support, and from being one big loop to being split into two main event-driven
components.

The design is by no means ideal, and refactoring it is an ongoing process. We've
generally focused on being pragmatic over trying to design everything perfectly
upfront, since nextest is partly an ongoing exploration of what a test runner
should be expected to do.

!!! info "Not overly generic!"

    A "natural" way to think of the nextest runner loop is as something that
    builds a DAG, then traverses it, and runs each unit of work whenever all its
    dependencies have completed.

    In practice, the runner loop is _not_ that generic, because there's a lot of
    specific logic around tests and scripts that would be hard to generalize. As
    new features are added, it's more straightforward to have a library of
    composable functions than to build a framework that can handle arbitrary
    units of work.

Nextest's core runner loop consists of two main components, the _dispatcher_ and
the _executor_. These components do not share state directly—instead, both
components are event-driven and use message passing to communicate with each
other.

## The dispatcher

The _dispatcher_ is the part of the runner that interacts with the outside world.
The dispatcher's job is to accept events from the following sources and respond
to them appropriately:

- The executor.
- Signal and input handlers.
- A handler that fires if the reporter produces an error.

Each iteration of the dispatcher loop has three phases:

1. **Select over sources.** Select over the event sources, generating an `InternalEvent`.

2. **Handle the event.** Based on the `InternalEvent`, do one or more of the following, in `handle_event`:

  - Update the dispatcher's internal state, such as the current number of tests
    running.
  - If a new unit of work is started, create and register a channel for the
    dispatcher to communicate with the unit.
  - Call the reporter to update the user interface.

    This call is currently synchronous; in practice, the reporter's stderr must
    be fully written out before the dispatcher can proceed. This is the simplest
    way to write the dispatcher, and is also a natural way of adding backpressure
    to the system (if stderr is backed up, the dispatcher will stop accepting new
    units of work). But it's worth re-evaluating if it ever becomes a bottleneck.

3. **Handle the response.** For some kinds of events, a `HandleEventResponse` is
   returned. A response can be any of:

  - Gather information from units.
  - On Unix platforms, stop or continue execution (on `SIGTSTP` and `SIGCONT` respectively).
  - If a condition for cancelling the run is met, cancel the run.

  Based on the response, the dispatcher broadcasts a request to all executing
  units of work. (This broadcast doesn't use a Tokio broadcast channel.
  Instead, each unit has a dedicated channel associated with it. This
  enables future improvements where only a subset of units are notified.)

### Linearizing events

The dispatcher's design focuses on *linearizing* events, with judicious use of
synchronization points between the dispatcher and the executor. For example, if
a new unit if work is started, the executor waits for the dispatcher to
acknowledge and register the new unit before proceeding.

The general goal of this kind of linearization is to ensure a good user
experience via a consistent view of the system.

* For example, an earlier iteration of the runner used a broadcast channel for
  the dispatcher to talk to units. Whenever new units of work were scheduled,
  the executor cloned the broadcast receiver and proceeded without synchronizing
  with the dispatcher.
* In that case, it is possible for info responses to be slightly out of sync
  with the state of the system reported by the dispatcher. In practice, this
  often manifested as a different number of running tests reported by the
  dispatcher (the "N running, M passed, ..." line in the UI) and the executor
  (the number of receivers the broadcast channel happened to have at that
  moment.)

The current architecture, where each unit of work gets a dedicated channel,
avoids this problem.

### The executor

The _executor_ is responsible for scheduling units of work and running them to
completion.

Each unit is a state machine written in async Rust. Units have two communication
channels, both with the dispatcher:

* A channel to send *responses* to the dispatcher, used to report progress, completion and errors.
* A channel to receive *requests* from the dispatcher, used for querying the unit's
  state, job control on Unix, and cancellation requests.

Units are the source of truth for their own state. For info queries, the
dispatcher by itself can say very little. Most of the information comes directly
from the units.

## The unit state machine

Units transition between several states over their lifetime. This is the
simplest unit lifecycle which manages [timeouts] and [leaky tests]:

``` mermaid
graph TB
  start[start] -->spawn{spawn}
  spawn -->|spawn successful| running{running};
  spawn -->|spawn failed| start_error[start error];
  running -->|process exited| cleanup{cleanup};
  cleanup -->|fds closed| success{success?};
  cleanup -->|fds leaked| leaky[mark leaky];
  leaky --> success;
  success -->|yes| mark_success[mark success];
  success -->|no| mark_failure[mark failure];
  running -->|timeout| start_termination{terminating};
  start_termination -->|exits within grace period| mark_timeout[mark timeout];
  start_termination -->|exceeds grace period| kill[kill];
  kill -->|process exits| mark_timeout;
```

[timeouts]: ../../features/slow-tests.md
[leaky tests]: ../../features/leaky-tests.md

For simplicity, this state diagram omits some details.

* With retries, the lifecycle above represents a single attempt to run a test.

  * If the test passes, it is currently not retried. (Some future work to do stress runs
    may change this.)
  * If the test fails in any way (timeout, non-zero exit code, start error, etc),
    and if any retries are left, the test may be retried after a possible delay.

* There are also incoming inputs (such as cancellation requests
  from the dispatcher) that can affect the unit's state. In reality, each step
  that performs a wait selects across:

  * The thing or things being waited for.
  * The request channel managed by the dispatcher.

  On receiving a message from the dispatcher, each step is responsible for responding
  to it based on its local state.

  * For example, if a step receives an information request, it
    responds with the state it's currently in and relevant state-specific
    information.
  * If the unit is currently being terminated, the response contains: why
    termination was requested (timeout or signal), how it is being done,
    how long the unit has been in that state, and how much time is left
    before it's killed).

### Error handling

The executor has specific logic for handling the various ways units can fail:

- The executable failing to start. (This is more common than you'd think! The most
  common reason is that the binary to execute has a different archiecture.)
- Exiting with a non-zero code, or aborts with a Unix signal or Windows abort code.
- The unit timing out.
- Reading from standard output or standard error returning an error.
- In case of a setup script, failing to parse the output of the script.

The full space of errors is modeled out in in the event types—no "shortcuts" are
taken. We've consistently found that modeling out the full space of errors
ensures a user experience that takes care of all the edge cases, and remains
sustainably high-quality over time.

**Getting the details right here is really important!** The primary job of a test
runner is to handle all of the ways tests can fail, just like the primary job of
a compiler is to handle all the ways users can write incorrect code.

## Signal handling

Signal handling uses Tokio's native support for signals. Signals are received by
a [multiplexer] called `SignalHandler`, which generates a stream of events. The
dispatcher then selects over this stream. On receiving a signal, the dispatcher
is responsible for broadcasting a message to all units.

[multiplexer]: https://docs.rs/nextest-runner/latest/nextest_runner/signal/index.html

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

### Unix process groups

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

### Windows job objects

On Windows, nextest uses [job objects] to manage the lifetime of all child
processes. Job objects don't support graceful termination like Unix signals do.
The only method available is [`TerminateJobObject`][terminate-job-object], which
is equivalent to a Unix `SIGKILL`.

[job objects]: https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
[terminate-job-object]: https://docs.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-terminatejobobject

### Shutdown signal handling

On receiving a signal like `SIGINT` or `SIGTERM`, or a Ctrl-C on Windows, the
dispatcher sends a message to all units to terminate with the same signal.
Units then follow a flow similar to timeouts, where they have a grace period to
exit cleanly before being killed.

Currently, nextest sends `SIGINT` on receiving `SIGINT`, `SIGTERM` on receiving
`SIGTERM`, and so on. There is no _inherent_ reason the two have to be the same,
other than a general expectation of "abstraction coherence": if you set up
process groups, you should behave similarly to the world in which you don't set
up process groups. This is a good principle to follow, but it's not a hard
requirement.

### Job control

On Unix platforms, nextest supports job control via the `SIGTSTP` and `SIGCONT`
signals.

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
spawn the child. Instead, it spawns a copy of itself, which then spawns the
process using `exec`.

This double-spawn approach works around a gnarly race with `SIGTSTP` handling.
If a child process receives `SIGTSTP` at exactly the wrong time (a window of
around 5-10 milliseconds on modern hardware), it can get stuck in a "half-born"
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
running `cargo nextest run -j64` against a repository; [nextest's own
repository] is a good candidate. Now try hitting `Ctrl-Z` a few times, running
`fg` to resume nextest after each pause. You'll likely see one run in ten or so
where nextest gets stuck.

With the double-spawn approach:

* Nextest first uses a [signal mask] to temporarily block `SIGTSTP`.
* It then spawns a copy of itself, which inherits the signal mask.
* At this point, the spawn is complete, and the parent process can carry on.
* The child nextest then unblocks `SIGTSTP`.

If a `SIGTSTP` is received at this point, the process is paused. But,
importantly, the parent does not get stuck waiting for the child to finish
spawning.

The double-spawn approach completely addresses this race, and no instances of a
stuck runner have been observed since it was implemented.

[nextest's own repository]: https://github.com/nextest-rs/nextest
[signal mask]: https://www.gnu.org/software/libc/manual/html_node/Process-Signal-Mask.html

_Last major revision: 2024-12-06_
