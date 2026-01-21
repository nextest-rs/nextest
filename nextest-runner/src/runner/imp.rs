// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{DispatcherContext, ExecutorContext, RunnerTaskState};
use crate::{
    config::{
        core::EvaluatableProfile,
        elements::{MaxFail, RetryPolicy, TestGroup, TestThreads},
        scripts::SetupScriptExecuteData,
    },
    double_spawn::DoubleSpawnInfo,
    errors::{
        ConfigureHandleInheritanceError, DebuggerCommandParseError, StressCountParseError,
        TestRunnerBuildError, TestRunnerExecuteErrors, TracerCommandParseError,
    },
    input::{InputHandler, InputHandlerKind, InputHandlerStatus},
    list::{OwnedTestInstanceId, TestInstanceWithSettings, TestList},
    reporter::events::{ReporterEvent, RunStats, StressIndex},
    runner::ExecutorEvent,
    signal::{SignalHandler, SignalHandlerKind},
    target_runner::TargetRunner,
    test_output::CaptureStrategy,
};
use async_scoped::TokioScope;
use chrono::{DateTime, Local};
use future_queue::{FutureQueueContext, StreamExt};
use futures::{future::Fuse, prelude::*};
use nextest_metadata::FilterMatch;
use quick_junit::ReportUuid;
use std::{
    collections::BTreeSet, convert::Infallible, fmt, num::NonZero, pin::Pin, str::FromStr,
    sync::Arc, time::Duration,
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc::unbounded_channel, oneshot},
    task::JoinError,
};
use tracing::{debug, warn};

/// A parsed debugger command.
#[derive(Clone, Debug)]
pub struct DebuggerCommand {
    program: String,
    args: Vec<String>,
}

impl DebuggerCommand {
    /// Gets the program.
    pub fn program(&self) -> &str {
        // The from_str constructor ensures that there is at least one part.
        &self.program
    }

    /// Gets the arguments.
    pub fn args(&self) -> &[String] {
        &self.args
    }
}

impl FromStr for DebuggerCommand {
    type Err = DebuggerCommandParseError;

    fn from_str(command: &str) -> Result<Self, Self::Err> {
        let mut parts =
            shell_words::split(command).map_err(DebuggerCommandParseError::ShellWordsParse)?;
        if parts.is_empty() {
            return Err(DebuggerCommandParseError::EmptyCommand);
        }
        let program = parts.remove(0);
        Ok(Self {
            program,
            args: parts,
        })
    }
}

/// A parsed tracer command.
#[derive(Clone, Debug)]
pub struct TracerCommand {
    program: String,
    args: Vec<String>,
}

impl TracerCommand {
    /// Gets the program.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Gets the arguments.
    pub fn args(&self) -> &[String] {
        &self.args
    }
}

impl FromStr for TracerCommand {
    type Err = TracerCommandParseError;

    fn from_str(command: &str) -> Result<Self, Self::Err> {
        let mut parts =
            shell_words::split(command).map_err(TracerCommandParseError::ShellWordsParse)?;
        if parts.is_empty() {
            return Err(TracerCommandParseError::EmptyCommand);
        }
        let program = parts.remove(0);
        Ok(Self {
            program,
            args: parts,
        })
    }
}

/// An interceptor wraps test execution with a debugger or tracer.
#[derive(Clone, Debug, Default)]
pub enum Interceptor {
    /// No interceptor - standard test execution.
    #[default]
    None,

    /// Run the test under a debugger.
    Debugger(DebuggerCommand),

    /// Run the test under a syscall tracer.
    Tracer(TracerCommand),
}

impl Interceptor {
    /// Returns true if timeouts should be disabled.
    ///
    /// Both debuggers and tracers disable timeouts.
    pub fn should_disable_timeouts(&self) -> bool {
        match self {
            Interceptor::None => false,
            Interceptor::Debugger(_) | Interceptor::Tracer(_) => true,
        }
    }

    /// Returns true if stdin should be passed through to child test processes.
    ///
    /// Only debuggers need stdin passthrough for interactive debugging.
    pub fn should_passthrough_stdin(&self) -> bool {
        match self {
            Interceptor::None | Interceptor::Tracer(_) => false,
            Interceptor::Debugger(_) => true,
        }
    }

    /// Returns true if a process group should be created for the child.
    ///
    /// Debuggers need terminal control, so no process group is created. Tracers
    /// work fine with process groups.
    pub fn should_create_process_group(&self) -> bool {
        match self {
            Interceptor::None | Interceptor::Tracer(_) => true,
            Interceptor::Debugger(_) => false,
        }
    }

    /// Returns true if leak detection should be skipped.
    ///
    /// Both debuggers and tracers skip leak detection to avoid interference.
    pub fn should_skip_leak_detection(&self) -> bool {
        match self {
            Interceptor::None => false,
            Interceptor::Debugger(_) | Interceptor::Tracer(_) => true,
        }
    }

    /// Returns true if the test command should be displayed.
    ///
    /// Used to determine if we should print the wrapper command for debugging.
    pub fn should_show_wrapper_command(&self) -> bool {
        match self {
            Interceptor::None => false,
            Interceptor::Debugger(_) | Interceptor::Tracer(_) => true,
        }
    }

    /// Returns true if, on receiving SIGTSTP, we should send SIGTSTP to the
    /// child.
    ///
    /// Debugger mode has special signal handling where we don't send SIGTSTP to
    /// the child (it receives it directly from the terminal, since no process
    /// group is created). Tracers use standard signal handling.
    pub fn should_send_sigtstp(&self) -> bool {
        match self {
            Interceptor::None | Interceptor::Tracer(_) => true,
            Interceptor::Debugger(_) => false,
        }
    }
}

/// A child process identifier: either a single process or a process group.
#[derive(Copy, Clone, Debug)]
pub(super) enum ChildPid {
    /// A single process ID.
    Process(#[cfg_attr(not(unix), expect(unused))] u32),

    /// A process group ID.
    #[cfg(unix)]
    ProcessGroup(u32),
}

impl ChildPid {
    /// Returns the PID value to use with `libc::kill`.
    ///
    /// - `Process(pid)` returns `pid as i32` (positive, kills single process).
    /// - `ProcessGroup(pid)` returns `-(pid as i32)` (negative, kills process group).
    ///
    /// On Windows, always returns `pid as i32`.
    #[cfg(unix)]
    pub(super) fn for_kill(self) -> i32 {
        match self {
            ChildPid::Process(pid) => pid as i32,
            ChildPid::ProcessGroup(pid) => -(pid as i32),
        }
    }
}

/// Test runner options.
#[derive(Debug, Default)]
pub struct TestRunnerBuilder {
    capture_strategy: CaptureStrategy,
    retries: Option<RetryPolicy>,
    max_fail: Option<MaxFail>,
    test_threads: Option<TestThreads>,
    stress_condition: Option<StressCondition>,
    interceptor: Interceptor,
    expected_outstanding: Option<BTreeSet<OwnedTestInstanceId>>,
}

impl TestRunnerBuilder {
    /// Sets the capture strategy for the test runner
    ///
    /// * [`CaptureStrategy::Split`]
    ///   * pro: output from `stdout` and `stderr` can be identified and easily split
    ///   * con: ordering between the streams cannot be guaranteed
    /// * [`CaptureStrategy::Combined`]
    ///   * pro: output is guaranteed to be ordered as it would in a terminal emulator
    ///   * con: distinction between `stdout` and `stderr` is lost
    /// * [`CaptureStrategy::None`] -
    ///   * In this mode, tests will always be run serially: `test_threads` will always be 1.
    pub fn set_capture_strategy(&mut self, strategy: CaptureStrategy) -> &mut Self {
        self.capture_strategy = strategy;
        self
    }

    /// Sets the number of retries for this test runner.
    pub fn set_retries(&mut self, retries: RetryPolicy) -> &mut Self {
        self.retries = Some(retries);
        self
    }

    /// Sets the max-fail value for this test runner.
    pub fn set_max_fail(&mut self, max_fail: MaxFail) -> &mut Self {
        self.max_fail = Some(max_fail);
        self
    }

    /// Sets the number of tests to run simultaneously.
    pub fn set_test_threads(&mut self, test_threads: TestThreads) -> &mut Self {
        self.test_threads = Some(test_threads);
        self
    }

    /// Sets the stress testing condition.
    pub fn set_stress_condition(&mut self, stress_condition: StressCondition) -> &mut Self {
        self.stress_condition = Some(stress_condition);
        self
    }

    /// Sets the interceptor (debugger or tracer) to use for running tests.
    pub fn set_interceptor(&mut self, interceptor: Interceptor) -> &mut Self {
        self.interceptor = interceptor;
        self
    }

    /// Sets the expected outstanding tests for rerun tracking.
    ///
    /// When set, the dispatcher will track which tests were seen during the run
    /// and emit a `TestsNotSeen` as part of the `RunFinished` if some expected
    /// tests were not seen.
    pub fn set_expected_outstanding(
        &mut self,
        expected: BTreeSet<OwnedTestInstanceId>,
    ) -> &mut Self {
        self.expected_outstanding = Some(expected);
        self
    }

    /// Creates a new test runner.
    #[expect(clippy::too_many_arguments)]
    pub fn build<'a>(
        self,
        test_list: &'a TestList,
        profile: &'a EvaluatableProfile<'a>,
        cli_args: Vec<String>,
        signal_handler: SignalHandlerKind,
        input_handler: InputHandlerKind,
        double_spawn: DoubleSpawnInfo,
        target_runner: TargetRunner,
    ) -> Result<TestRunner<'a>, TestRunnerBuildError> {
        let test_threads = match self.capture_strategy {
            CaptureStrategy::None => 1,
            CaptureStrategy::Combined | CaptureStrategy::Split => self
                .test_threads
                .unwrap_or_else(|| profile.test_threads())
                .compute(),
        };
        let max_fail = self.max_fail.unwrap_or_else(|| profile.max_fail());

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("nextest-runner-worker")
            .build()
            .map_err(TestRunnerBuildError::TokioRuntimeCreate)?;
        let _guard = runtime.enter();

        // signal_handler.build() must be called from within the guard.
        let signal_handler = signal_handler.build()?;

        let input_handler = input_handler.build();

        Ok(TestRunner {
            inner: TestRunnerInner {
                run_id: force_or_new_run_id(),
                started_at: Local::now(),
                profile,
                test_list,
                test_threads,
                double_spawn,
                target_runner,
                capture_strategy: self.capture_strategy,
                force_retries: self.retries,
                cli_args,
                max_fail,
                stress_condition: self.stress_condition,
                interceptor: self.interceptor,
                expected_outstanding: self.expected_outstanding,
                runtime,
            },
            signal_handler,
            input_handler,
        })
    }
}

/// Stress testing condition.
#[derive(Clone, Debug)]
pub enum StressCondition {
    /// Run each test `count` times.
    Count(StressCount),

    /// Run until this duration has elapsed.
    Duration(Duration),
}

/// A count for stress testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum StressCount {
    /// Run each test `count` times.
    Count {
        /// The number of times to run each test.
        count: NonZero<u32>,
    },

    /// Run indefinitely.
    Infinite,
}

impl FromStr for StressCount {
    type Err = StressCountParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "infinite" {
            Ok(StressCount::Infinite)
        } else {
            match s.parse() {
                Ok(count) => Ok(StressCount::Count { count }),
                Err(_) => Err(StressCountParseError::new(s)),
            }
        }
    }
}

/// Context for running tests.
///
/// Created using [`TestRunnerBuilder::build`].
#[derive(Debug)]
pub struct TestRunner<'a> {
    inner: TestRunnerInner<'a>,
    signal_handler: SignalHandler,
    input_handler: InputHandler,
}

impl<'a> TestRunner<'a> {
    /// Returns the unique ID for this test run.
    pub fn run_id(&self) -> ReportUuid {
        self.inner.run_id
    }

    /// Returns the timestamp when this test run was started.
    pub fn started_at(&self) -> DateTime<Local> {
        self.inner.started_at
    }

    /// Returns the status of the input handler.
    pub fn input_handler_status(&self) -> InputHandlerStatus {
        self.input_handler.status()
    }

    /// Executes the listed tests, each one in its own process.
    ///
    /// The callback is called with the results of each test.
    ///
    /// Returns an error if any of the tasks panicked.
    pub fn execute<F>(
        self,
        mut callback: F,
    ) -> Result<RunStats, TestRunnerExecuteErrors<Infallible>>
    where
        F: FnMut(ReporterEvent<'a>) + Send,
    {
        self.try_execute::<Infallible, _>(|event| {
            callback(event);
            Ok(())
        })
    }

    /// Executes the listed tests, each one in its own process.
    ///
    /// Accepts a callback that is called with the results of each test. If the callback returns an
    /// error, the test run terminates and the callback is no longer called.
    ///
    /// Returns an error if any of the tasks panicked.
    pub fn try_execute<E, F>(
        mut self,
        mut callback: F,
    ) -> Result<RunStats, TestRunnerExecuteErrors<E>>
    where
        F: FnMut(ReporterEvent<'a>) -> Result<(), E> + Send,
        E: fmt::Debug + Send,
    {
        let (report_cancel_tx, report_cancel_rx) = oneshot::channel();

        // If report_cancel_tx is None, at least one error has occurred and the
        // runner has been instructed to shut down. first_error is also set to
        // Some in that case.
        let mut report_cancel_tx = Some(report_cancel_tx);
        let mut first_error = None;

        let res = self.inner.execute(
            &mut self.signal_handler,
            &mut self.input_handler,
            report_cancel_rx,
            |event| {
                match callback(event) {
                    Ok(()) => {}
                    Err(error) => {
                        // If the callback fails, we need to let the runner know to start shutting
                        // down. But we keep reporting results in case the callback starts working
                        // again.
                        if let Some(report_cancel_tx) = report_cancel_tx.take() {
                            let _ = report_cancel_tx.send(());
                            first_error = Some(error);
                        }
                    }
                }
            },
        );

        // On Windows, the stdout and stderr futures might spawn processes that keep the runner
        // stuck indefinitely if it's dropped the normal way. Shut it down aggressively, being OK
        // with leaked resources.
        self.inner.runtime.shutdown_background();

        match (res, first_error) {
            (Ok(run_stats), None) => Ok(run_stats),
            (Ok(_), Some(report_error)) => Err(TestRunnerExecuteErrors {
                report_error: Some(report_error),
                join_errors: Vec::new(),
            }),
            (Err(join_errors), report_error) => Err(TestRunnerExecuteErrors {
                report_error,
                join_errors,
            }),
        }
    }
}

#[derive(Debug)]
struct TestRunnerInner<'a> {
    run_id: ReportUuid,
    started_at: DateTime<Local>,
    profile: &'a EvaluatableProfile<'a>,
    test_list: &'a TestList<'a>,
    test_threads: usize,
    double_spawn: DoubleSpawnInfo,
    target_runner: TargetRunner,
    capture_strategy: CaptureStrategy,
    force_retries: Option<RetryPolicy>,
    cli_args: Vec<String>,
    max_fail: MaxFail,
    stress_condition: Option<StressCondition>,
    interceptor: Interceptor,
    expected_outstanding: Option<BTreeSet<OwnedTestInstanceId>>,
    runtime: Runtime,
}

impl<'a> TestRunnerInner<'a> {
    fn execute<F>(
        &self,
        signal_handler: &mut SignalHandler,
        input_handler: &mut InputHandler,
        report_cancel_rx: oneshot::Receiver<()>,
        callback: F,
    ) -> Result<RunStats, Vec<JoinError>>
    where
        F: FnMut(ReporterEvent<'a>) + Send,
    {
        // TODO: add support for other test-running approaches, measure performance.

        // Disable the global timeout when an interceptor is active.
        let global_timeout = if self.interceptor.should_disable_timeouts() {
            crate::time::far_future_duration()
        } else {
            self.profile.global_timeout(self.test_list.mode()).period
        };

        let mut dispatcher_cx = DispatcherContext::new(
            callback,
            self.run_id,
            self.profile.name(),
            self.cli_args.clone(),
            self.test_list.run_count(),
            self.max_fail,
            global_timeout,
            self.stress_condition.clone(),
            self.expected_outstanding.clone(),
        );

        let executor_cx = ExecutorContext::new(
            self.run_id,
            self.profile,
            self.test_list,
            self.double_spawn.clone(),
            self.target_runner.clone(),
            self.capture_strategy,
            self.force_retries,
            self.interceptor.clone(),
        );

        // Send the initial event.
        dispatcher_cx.run_started(self.test_list, self.test_threads);

        let _guard = self.runtime.enter();

        let mut report_cancel_rx = std::pin::pin!(report_cancel_rx.fuse());

        if self.stress_condition.is_some() {
            loop {
                let progress = dispatcher_cx
                    .stress_progress()
                    .expect("stress_condition is Some => stress progress is Some");
                if progress.remaining().is_some() {
                    dispatcher_cx.stress_sub_run_started(progress);

                    self.do_run(
                        dispatcher_cx.stress_index(),
                        &mut dispatcher_cx,
                        &executor_cx,
                        signal_handler,
                        input_handler,
                        report_cancel_rx.as_mut(),
                    )?;

                    dispatcher_cx.stress_sub_run_finished();

                    if dispatcher_cx.cancel_reason().is_some() {
                        break;
                    }
                } else {
                    break;
                }
            }
        } else {
            self.do_run(
                None,
                &mut dispatcher_cx,
                &executor_cx,
                signal_handler,
                input_handler,
                report_cancel_rx,
            )?;
        }

        let run_stats = dispatcher_cx.run_stats();
        dispatcher_cx.run_finished();

        Ok(run_stats)
    }

    fn do_run<F>(
        &self,
        stress_index: Option<StressIndex>,
        dispatcher_cx: &mut DispatcherContext<'a, F>,
        executor_cx: &ExecutorContext<'a>,
        signal_handler: &mut SignalHandler,
        input_handler: &mut InputHandler,
        report_cancel_rx: Pin<&mut Fuse<oneshot::Receiver<()>>>,
    ) -> Result<(), Vec<JoinError>>
    where
        F: FnMut(ReporterEvent<'a>) + Send,
    {
        let ((), results) = TokioScope::scope_and_block(move |scope| {
            let (resp_tx, resp_rx) = unbounded_channel::<ExecutorEvent<'a>>();

            // Run the dispatcher to completion in a task.
            let dispatcher_fut =
                dispatcher_cx.run(resp_rx, signal_handler, input_handler, report_cancel_rx);
            scope.spawn_cancellable(dispatcher_fut, || RunnerTaskState::Cancelled);

            let (script_tx, mut script_rx) = unbounded_channel::<SetupScriptExecuteData<'a>>();
            let script_resp_tx = resp_tx.clone();
            let run_scripts_fut = async move {
                // Since script tasks are run serially, we just reuse the one
                // script task.
                let script_data = executor_cx
                    .run_setup_scripts(stress_index, script_resp_tx)
                    .await;
                if script_tx.send(script_data).is_err() {
                    // The dispatcher has shut down, so we should too.
                    debug!("script_tx.send failed, shutting down");
                }
                RunnerTaskState::finished_no_children()
            };
            scope.spawn_cancellable(run_scripts_fut, || RunnerTaskState::Cancelled);

            let Some(script_data) = script_rx.blocking_recv() else {
                // Most likely the harness is shutting down, so we should too.
                debug!("no script data received, shutting down");
                return;
            };

            // groups is going to be passed to future_queue_grouped.
            let groups = self
                .profile
                .test_group_config()
                .iter()
                .map(|(group_name, config)| (group_name, config.max_threads.compute()));

            let setup_script_data = Arc::new(script_data);

            let filter_resp_tx = resp_tx.clone();

            let tests = self.test_list.to_priority_queue(self.profile);
            let run_tests_fut = futures::stream::iter(tests)
                .filter_map(move |test| {
                    // Filter tests before assigning a FutureQueueContext to
                    // them.
                    //
                    // Note that this function is called lazily due to the
                    // `future_queue_grouped` below. This means that skip
                    // notifications will go out as tests are iterated over, not
                    // all at once.
                    let filter_resp_tx = filter_resp_tx.clone();
                    async move {
                        if let FilterMatch::Mismatch { reason } =
                            test.instance.test_info.filter_match
                        {
                            // Failure to send means the receiver was dropped.
                            let _ = filter_resp_tx.send(ExecutorEvent::Skipped {
                                stress_index,
                                test_instance: test.instance,
                                reason,
                            });
                            return None;
                        }
                        Some(test)
                    }
                })
                .map(move |test: TestInstanceWithSettings<'a>| {
                    let threads_required =
                        test.settings.threads_required().compute(self.test_threads);
                    let test_group = match test.settings.test_group() {
                        TestGroup::Global => None,
                        TestGroup::Custom(name) => Some(name.clone()),
                    };
                    let resp_tx = resp_tx.clone();
                    let setup_script_data = setup_script_data.clone();

                    let test_instance = test.instance;

                    let f = move |cx: FutureQueueContext| {
                        debug!("running test instance: {}; cx: {cx:?}", test_instance.id());
                        // Use a separate Tokio task for each test. For repos
                        // with lots of small tests, this has been observed to
                        // be much faster than using a single task for all tests
                        // (what we used to do). It also provides some degree of
                        // per-test isolation.
                        async move {
                            // SAFETY: Within an outer scope_and_block (which we
                            // have here), scope_and_collect is safe as long as
                            // the returned future isn't forgotten. We're not
                            // forgetting it below -- we're running it to
                            // completion immediately.
                            //
                            // But recursive scoped calls really feel like
                            // pushing against the limits of async-scoped. For
                            // example, there's no way built into async-scoped
                            // to propagate a cancellation signal from the outer
                            // scope to the inner scope. (But there could be,
                            // right? That seems solvable via channels. And we
                            // could likely do our own channels here.)
                            let ((), mut ret) = unsafe {
                                TokioScope::scope_and_collect(move |scope| {
                                    scope.spawn(executor_cx.run_test_instance(
                                        stress_index,
                                        test,
                                        cx,
                                        resp_tx.clone(),
                                        setup_script_data,
                                    ))
                                })
                            }
                            .await;

                            // If no future was started, that's really strange.
                            // Worth at least logging.
                            let Some(result) = ret.pop() else {
                                warn!(
                                    "no task was started for test instance: {}",
                                    test_instance.id()
                                );
                                return None;
                            };
                            result.err()
                        }
                    };

                    (threads_required, test_group, f)
                })
                // future_queue_grouped means tests are spawned in the order
                // defined, but returned in any order.
                .future_queue_grouped(self.test_threads, groups)
                // Drop the None values.
                .filter_map(std::future::ready)
                .collect::<Vec<_>>()
                // Interestingly, using a more idiomatic `async move {
                // run_tests_fut.await ... }` block causes Rust 1.83 to complain
                // about a weird lifetime mismatch. FutureExt::map as used below
                // does not.
                .map(|child_join_errors| RunnerTaskState::Finished { child_join_errors });

            scope.spawn_cancellable(run_tests_fut, || RunnerTaskState::Cancelled);
        });

        // Were there any join errors in tasks?
        //
        // If one of the tasks panics, we likely end up stuck because the
        // dispatcher, which is spawned in the same async-scoped block, doesn't
        // get relayed the panic immediately. That should probably be fixed at
        // some point.
        let mut cancelled_count = 0;
        let join_errors = results
            .into_iter()
            .flat_map(|r| {
                match r {
                    Ok(RunnerTaskState::Finished { child_join_errors }) => child_join_errors,
                    // Largely ignore cancelled tasks since it most likely means
                    // shutdown -- we don't cancel tasks manually.
                    Ok(RunnerTaskState::Cancelled) => {
                        cancelled_count += 1;
                        Vec::new()
                    }
                    Err(join_error) => vec![join_error],
                }
            })
            .collect::<Vec<_>>();

        if cancelled_count > 0 {
            debug!(
                "{} tasks were cancelled -- this \
                 generally should only happen due to panics",
                cancelled_count
            );
        }
        if !join_errors.is_empty() {
            return Err(join_errors);
        }

        Ok(())
    }
}

/// Configures stdout, stdin and stderr inheritance by test processes on Windows.
///
/// With Rust on Windows, these handles can be held open by tests (and therefore by grandchild processes)
/// even if we run the tests with `Stdio::inherit`. This can cause problems with leaky tests.
///
/// This changes global state on the Win32 side, so the application must manage mutual exclusion
/// around it. Call this right before [`TestRunner::try_execute`].
///
/// This is a no-op on non-Windows platforms.
///
/// See [this issue on the Rust repository](https://github.com/rust-lang/rust/issues/54760) for more
/// discussion.
pub fn configure_handle_inheritance(
    no_capture: bool,
) -> Result<(), ConfigureHandleInheritanceError> {
    super::os::configure_handle_inheritance_impl(no_capture)
}

/// Environment variable to force a specific run ID (for testing).
const FORCE_RUN_ID_ENV: &str = "__NEXTEST_FORCE_RUN_ID";

/// Returns a forced run ID from the environment, or generates a new one.
fn force_or_new_run_id() -> ReportUuid {
    if let Ok(id_str) = std::env::var(FORCE_RUN_ID_ENV) {
        match id_str.parse::<ReportUuid>() {
            Ok(uuid) => return uuid,
            Err(err) => {
                warn!(
                    "{FORCE_RUN_ID_ENV} is set but invalid (expected UUID): {err}, \
                     generating random ID"
                );
            }
        }
    }
    ReportUuid::new_v4()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::core::NextestConfig, platform::BuildPlatforms};

    #[test]
    fn no_capture_settings() {
        // Ensure that output settings are ignored with no-capture.
        let mut builder = TestRunnerBuilder::default();
        builder
            .set_capture_strategy(CaptureStrategy::None)
            .set_test_threads(TestThreads::Count(20));
        let test_list = TestList::empty();
        let config = NextestConfig::default_config("/fake/dir");
        let profile = config.profile(NextestConfig::DEFAULT_PROFILE).unwrap();
        let build_platforms = BuildPlatforms::new_with_no_target().unwrap();
        let signal_handler = SignalHandlerKind::Noop;
        let input_handler = InputHandlerKind::Noop;
        let profile = profile.apply_build_platforms(&build_platforms);
        let runner = builder
            .build(
                &test_list,
                &profile,
                vec![],
                signal_handler,
                input_handler,
                DoubleSpawnInfo::disabled(),
                TargetRunner::empty(),
            )
            .unwrap();
        assert_eq!(runner.inner.capture_strategy, CaptureStrategy::None);
        assert_eq!(runner.inner.test_threads, 1, "tests run serially");
    }

    #[test]
    fn test_debugger_command_parsing() {
        // Valid commands
        let cmd = DebuggerCommand::from_str("gdb --args").unwrap();
        assert_eq!(cmd.program(), "gdb");
        assert_eq!(cmd.args(), &["--args"]);

        let cmd = DebuggerCommand::from_str("rust-gdb -ex run --args").unwrap();
        assert_eq!(cmd.program(), "rust-gdb");
        assert_eq!(cmd.args(), &["-ex", "run", "--args"]);

        // With quotes
        let cmd = DebuggerCommand::from_str(r#"gdb -ex "set print pretty on" --args"#).unwrap();
        assert_eq!(cmd.program(), "gdb");
        assert_eq!(cmd.args(), &["-ex", "set print pretty on", "--args"]);

        // Empty command
        let err = DebuggerCommand::from_str("").unwrap_err();
        assert!(matches!(err, DebuggerCommandParseError::EmptyCommand));

        // Whitespace only
        let err = DebuggerCommand::from_str("   ").unwrap_err();
        assert!(matches!(err, DebuggerCommandParseError::EmptyCommand));
    }
}
