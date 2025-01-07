---
icon: material/dns-outline
description: Why nextest runs each test in its own process, explained via game theory.
---

# Why process-per-test?

A key factor distinguishing nextest from `cargo test` is that nextest runs **each test in a separate process**. With nextest, the default execution model is now, and will always be, process-per-test. This document explains why.

Some terms used in this document:

* *Process-per-test:* Each test lives in its own process.
* *Shared-process:* Several tests share the same process.

## The ecosystem coordination challenge

The Rust ecosystem is now staggeringly large. There are millions of Rust developers and billions of lines of Rust code in production today. A key challenge with driving adoption of any new technology, like a new test runner, is **coordination**: everyone needs to agree on how to run and manage them.

* There are vast numbers of existing tests, representing millions of developer-hours of effort, that must continue to work
* These tests integrate with all sorts of different libraries, including many not written in Rust, or with other unusual requirements
* Test runners need to be able to run tests reliably, and rely on certain properties—for example, that killing one test will have no effect on others

## Focal points in game theory

<figure markdown="span">
  ![Photo of Nobel laureate Thomas Schelling, who first described focal points.](../../static/schelling.jpg){ width="400" }
  <figcaption markdown="span">Thomas Schelling won the 2005 Nobel Prize in Economics for his work on focal points. [Boston Globe](https://www.bostonglobe.com/metro/obituaries/2016/12/17/thomas-schelling-nobel-winning-economist-who-influenced-nuclear-policy/1iMPQdz8NQFB75HwAPwWdM/story.html)</figcaption>
</figure>

[Game theory](https://en.wikipedia.org/wiki/Game_theory) offers a powerful framework to study these kinds of coordination problems. Consider this classic problem: if two people need to meet in London on a particular day but can't coordinate the time and place, they're likely to choose noon at [Big Ben](https://en.wikipedia.org/wiki/Big_Ben). This becomes what game theorists call a [*focal point*](https://en.wikipedia.org/wiki/Focal_point_(game_theory)), also known as a *Schelling point*—a natural default that everyone can assume without discussion.

The process-per-test model is powerful because **it serves as the focal point**, the Big Ben, of how to run tests. Just as everyone in London knows where the Great Clock of Westminster is, every operating system knows how to create and manage processes. Just as noon is a natural meeting time, processes form natural fault boundaries. The process-per-test model forms a universal protocol that everyone in the ecosystem can rely on without explicit coordination.

## The benefits of separate processes

This game-theoretic lens provides fresh insight into the benefits of process-per-test:

* **Test authors and libraries don't need to coordinate global state usage.** There are a surprising number of real-world use cases that *require* per-test isolation—particularly integration tests against global state:

  * Tests against a graphical API like [EGL](https://www.khronos.org/egl), which only allows one thread in a process to create a GPU context. [`wgpu`](https://github.com/gfx-rs/wgpu) uses nextest for this reason
  * Tests that [must be run on the main thread](../configuration/extra-args.md) of their corresponding process, like tests against certain macOS frameworks
  * Tests that require altering environment variables or other global context

  Even if particular tests don't *require* per-test isolation, they may often *benefit* from it. For example, global in-memory state becomes separated per-test. (You may argue that global state isn't desirable, or should not be used for tests—but instead, think of per-test isolation as a principled, zero-coordination solution to this class of issues.)

* **Internal memory handling doesn't need to be coordinated across tests.** Memory corruption in one test doesn't cause others to behave erratically. One test segfaulting does not take down a bunch of other tests.

* **Test runners don't need a custom protocol.** With process-per-test, test runners can rely on universal OS primitives, like the ability to kill processes. For more discussion, see [*The costs of coordination*](#the-costs-of-coordination) below.

Process-per-test is not free, though. It's worth acknowledging some costs:

* Tests might want to share in-memory state. They might have an in-memory semaphore for rate-limiting, a shared in-memory immutable cache that is primed on first access, or an in-memory server to which tests communicate via message passing. Process-per-test makes these valuable patterns **harder to achieve**: semaphores must [be managed by](../configuration/test-groups.md) the test runner, in-memory state must be stored on disk, and shared servers must live out-of-process. Adapting existing tests to process-per-test may require a significant engineering effort.

* Another downside that's noticeable in some situations is process creation **performance**. Creating processes is very fast on Linux and most other Unix-like systems. It is quite slow on Windows, though, and it can be slow on macOS [if anti-malware protections are interfering](../installation/macos.md#gatekeeper).

## The costs of coordination

Process-per-test enables rich lifecycle management with zero coordination via universal OS primitives. What kinds of coordination would enable shared-process tests?

Here's a partial list of operations that nextest performs using OS primitives:

1. **Start a test.** In particular, start a test at a specific moment, and not before then; and also, do not start a test if it is filtered out or the run is cancelled. With a process-per-test model, this is natural: simply start the test process.
2. **Know when a test is done, as it's done.** With process-per-test, wait for the process to exit.
3. **Measure test times.** Not just after a test is done, but while it is running. With process-per-test, use wall-clock time.
4. **Terminate tests that timed out.** In particular, one test timing out should not cause others to be killed. With process-per-test, this is [solved](architecture/signal-handling.md) via signals on Unix, and process groups on Windows.
5. **Retry failed tests.** This is an extension to point 1: retrying tests, and marking tests as flaky if they succeeded later.
6. **Cancel tests on an input.** On a test failure or receiving a signal, the test runner may decide to leave tests running but no longer schedule new ones, or to cancel existing tests.
7. **Gather test output.** With process-per-test, nextest can read standard output and standard error for the process.

Compare this to `cargo test`'s shared-process model. `cargo test` works by embedding a small runner into each test binary which manages rate-limiting and filtering.

* However, as of early 2025, each runner works in isolation: there is no global orchestrator that can schedule tests across multiple binaries.

* There is also no way to configure timeouts for tests, or to cancel some (but not all) running tests. For more, see [the appendix](#appendix).

* If a test takes down the process, all of the other tests in the binary are effectively cancelled as well. In this case, getting a full accounting of all tests and what happened to them is extremely difficult. [JUnit reports](../machine-readable/junit.md) wouldn't be nearly as valuable if they were missing many tests!

So, effective and robust management of tests in the shared-process model requires coordination between three entities:

* **The test runner:** There must be an overall orchestrator, and for robustness it must live in a separate process from any of the test binaries.
* **A component within the test binary:** These requirements preclude any kind of one-shot model where the list of tests is provided upfront and run to completion. Instead, there must be an in-memory component that's dynamic and event-driven. There needs to be a rich, bidirectional communication protocol between the test runner and this component. (Simpler protocols like [the GNU make jobserver](https://docs.rs/jobserver) can do rate-limiting, but not cancellation, retries, or time measurement.)
* **The test itself:** For test cancellation in particular, there must be some amount of cooperation from the test itself. Either the test needs to periodically check for a cancellation flag, or it must use async Rust that can be cancelled at the next await point. For more on why, see [the appendix](#appendix).

This represents a *lot* of extra work! Not just the technical kind (though that is certainly quite involved), but also the coordination kind: building support for such a protocol and getting developers to adopt it. And it would still not solve every problem. (For example, segfaults would still take down the whole process.)

## Conclusion

<figure markdown="span">
  ![Photo of the clock on the Great Clock of Westminster, also known as Big Ben.](../../static/big-ben.jpg){ width="400" }
  <figcaption markdown="span">Big Ben, the most recognizable landmark in London. [Unsplash / Henry Be](https://unsplash.com/photos/big-ben-london-MdJq0zFUwrw)</figcaption>
</figure>

There are many technical benefits to the process-per-test model, but the biggest benefit is in (the lack of) coordination: the process-per-test model acts as a focal point that all participants can agree on by default. This is the key reason that nextest commits to the process-per-test model being the default in perpetuity.

Nevertheless, we're excited to see newer developments in this space, and will consider adopting newer patterns that can deliver feature-rich and reliable test running at scale like nextest does today.

## Appendix: thread cancellation is hard { #appendix }

Many nextest users rely on its ability to cancel running tests due to timeouts. This section talks about why terminating threads is much more difficult than terminating processes.

With process-per-test, nextest sends a `SIGTERM` signal to the test process, and if it doesn't exit within 10 seconds, a `SIGKILL`.

With a shared-process model, assuming each test is in a separate thread, a timeout would require the thread to be killed rather than the process. Unlike processes, though, threads lack well-defined isolation, making their termination far more hazardous and unpredictable.

For example, the Windows [`TerminateThread`][terminate-thread] function explicitly warns against ever using it:

> TerminateThread is a dangerous function that should only be used in the most extreme cases. You should call TerminateThread only if you know exactly what the target thread is doing, and you control all of the code that the target thread could possibly be running at the time of the termination.

As an example, with `TerminateThread` and the equivalent POSIX [`pthread_cancel`](https://man7.org/linux/man-pages/man3/pthread_cancel.3.html), the behavior of synchronization primitives like `std::sync::Mutex` is undefined. Certainly, Rust mutexes will not be marked [poisoned](https://doc.rust-lang.org/beta/std/sync/struct.Mutex.html#poisoning). It's possible that mutexes are even held forever and never released. The usage of thread-killing operations would be devastating to test reliability.

Practically speaking, the only reliable way to cancel synchronous Rust code is with some kind of cooperation from the test itself. This could mean the test checking a flag every so often, or by the test regularly calling into a library that panics with a special payload when it's time to cancel the test.

With async Rust, cancellation is [somewhat easier](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html#method.abort), because yield points are cancellation points. However, in an eerie similarity with `pthread_cancel`, Tokio mutexes are [*also* not marked poisoned](https://docs.rs/cancel-safe-futures/0.1/cancel_safe_futures/sync/struct.RobustMutex.html) on a task or future cancellation[^no-tokio-mutex].

It can be illuminating to work through why terminating processes is safe and routine, while terminating threads is dangerous.

* Threads do not generally expect other threads to just disappear without warning, but processes are expected to sometimes exit abnormally.
* Synchronization across processes is possible via functions like [`flock`][flock], but is often advisory and generally uncommon. Code that uses these tools is generally prepared to encounter protected data in an invalid state.
* Cross-process mutexes are quite rare (message-passing is much more common), and shared-memory code is usually written with great care.

These examples make clear how focal points manifest and perpetuate: we've all generally agreed that processes might behave in strange ways, but we assume that threads within our processes are going to behave reliably.

[^no-tokio-mutex]: Based on our experiences with cancellation in other Rust projects, we strongly recommend projects treat Tokio mutexes as a feature of last resort.

[terminate-thread]: https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminatethread
[flock]: http://linux.die.net/man/2/flock
