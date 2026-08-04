// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Running a Buck2 test session.
//!
//! ## Threads
//!
//! Three thread contexts are in play, and which code runs where is load-bearing
//! rather than incidental:
//!
//! * A tokio runtime owns both gRPC connections. It has worker threads of its
//!   own, so the connections keep making progress while the main thread is
//!   busy running tests.
//! * The main thread drives the run. It calls `block_on` for each phase that
//!   talks to Buck2, but is outside the runtime by the time tests start --
//!   which it must be, since [`TestRunner::try_execute`] builds a runtime of its
//!   own and would panic if nested inside one.
//! * A reporting thread, owned by [`ResultSink`], drains results and sends them.
//!   See that module for why the callback cannot send them itself.
//!
//! ## Phases
//!
//! 1. Connect to the orchestrator, and serve the executor service.
//! 2. Collect specs until Buck2 says there are no more.
//! 3. Ask Buck2 how to run each one.
//! 4. Run them, streaming results back as they finish.
//! 5. Report the exit code and stop.
//!
//! [`TestRunner::try_execute`]: nextest_runner::runner::TestRunner::try_execute

use super::{
    prepare::prepare_all,
    service::SpecCollector,
    sink::ResultSink,
    transport::{Socket, SocketSpec, serve_test_executor},
};
use crate::{
    cli::FilterOpts,
    convert::to_binary_list,
    errors::{ExpectedError, Result},
    proto::{
        ConfiguredTargetHandle, EndOfTestResultsRequest, test_executor_server::TestExecutorServer,
        test_orchestrator_client::TestOrchestratorClient,
    },
    run::RunContext,
};
use camino::Utf8PathBuf;
use nextest_metadata::{NextestExitCode, RustBinaryId};
use nextest_runner::platform::BuildPlatforms;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tokio::{runtime::Runtime, sync::oneshot};
use tonic::transport::Channel;

/// Everything the executor needs that did not come from Buck2.
#[derive(Debug)]
pub struct ExecutorOptions {
    /// The Buck2 project root.
    pub project_root: Utf8PathBuf,

    /// The nextest profile to use.
    pub profile_name: Option<String>,

    /// A path to a nextest configuration file, if one was given.
    pub config_file: Option<Utf8PathBuf>,

    /// Filters from the arguments after `--`.
    pub filter: FilterOpts,

    /// Environment variables to add to every test process.
    ///
    /// These come from `buck2 test ... -- --env NAME=VALUE`. They are applied
    /// on top of what Buck2 says each target needs, so a variable named in both
    /// places takes the value given here.
    pub extra_env: BTreeMap<String, String>,

    /// The full command line, recorded with the run.
    pub cli_args: Vec<String>,
}

/// Runs a session against Buck2, returning the process exit code.
pub fn exec(
    executor_socket: SocketSpec,
    orchestrator_socket: SocketSpec,
    options: ExecutorOptions,
) -> Result<i32> {
    // Two workers: one is enough to drive the connections, and a second keeps a
    // slow call from stalling the other socket.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("buck2-nextest-grpc")
        .build()
        .map_err(|error| ExpectedError::RuntimeCreateError { error })?;

    let session = runtime.block_on(collect(executor_socket, orchestrator_socket))?;
    let prepared = runtime.block_on(prepare(session))?;

    // From here on the main thread is outside the runtime, which is what lets
    // the runner build its own.
    run(&runtime, prepared, options)
}

/// The pieces phase 1 produces.
struct Session {
    client: TestOrchestratorClient<Channel>,
    specs: Vec<crate::proto::ExternalRunnerSpec>,
    /// Held so the executor service keeps serving; dropping it shuts it down.
    _shutdown: oneshot::Sender<()>,
}

async fn collect(executor_socket: SocketSpec, orchestrator_socket: SocketSpec) -> Result<Session> {
    let executor_socket = Socket::adopt(executor_socket, "executor").await?;
    let orchestrator_socket = Socket::adopt(orchestrator_socket, "orchestrator").await?;

    let channel = orchestrator_socket.into_channel("orchestrator").await?;
    let client = TestOrchestratorClient::new(channel);

    let (collector, specs_rx) = SpecCollector::new();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let service = TestExecutorServer::new(collector);

    // The service runs as a task so that this function can await the specs it
    // produces. Serving ends when Buck2 disconnects or `shutdown_tx` drops.
    let serving = tokio::spawn(serve_test_executor(executor_socket, service, async {
        let _ = shutdown_rx.await;
    }));

    let specs = match specs_rx.await {
        Ok(specs) => specs,
        // The sender is only dropped without sending if serving ended first,
        // which means Buck2 hung up. Surface that error rather than the
        // symptom.
        Err(_) => {
            return match serving.await {
                Ok(Ok(())) | Err(_) => Err(ExpectedError::Buck2Disconnected),
                Ok(Err(error)) => Err(error),
            };
        }
    };

    Ok(Session {
        client,
        specs,
        _shutdown: shutdown_tx,
    })
}

/// The pieces phase 3 produces.
struct Prepared {
    client: TestOrchestratorClient<Channel>,
    targets: Vec<crate::spec::Buck2TestTarget>,
    handles: HashMap<RustBinaryId, ConfiguredTargetHandle>,
    _shutdown: oneshot::Sender<()>,
}

async fn prepare(session: Session) -> Result<Prepared> {
    let Session {
        mut client,
        specs,
        _shutdown,
    } = session;

    let prepared = prepare_all(&mut client, specs).await?;

    let mut targets = Vec::with_capacity(prepared.len());
    let mut handles = HashMap::with_capacity(prepared.len());
    for item in prepared {
        // The binary ID is derived from the label, exactly as `to_binary_list`
        // derives it, so results can be matched back to their target.
        handles.insert(RustBinaryId::new(&item.target.label), item.handle);
        targets.push(item.target);
    }

    Ok(Prepared {
        client,
        targets,
        handles,
        _shutdown,
    })
}

fn run(runtime: &Runtime, prepared: Prepared, options: ExecutorOptions) -> Result<i32> {
    let Prepared {
        client,
        mut targets,
        handles,
        _shutdown,
    } = prepared;

    for target in &mut targets {
        target.env.extend(
            options
                .extra_env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
    }

    let build_platforms = BuildPlatforms::new_with_no_target().map_err(|error| {
        ExpectedError::HostPlatformDetectError {
            error: Box::new(error),
        }
    })?;
    let binaries = to_binary_list(&targets, &options.project_root, build_platforms);

    let cx = RunContext {
        binaries,
        project_root: options.project_root,
        profile_name: options.profile_name,
        config_file: options.config_file,
        filtersets: options.filter.filtersets(),
        filter_patterns: options.filter.patterns(),
        run_ignored: options.filter.run_ignored(),
        list_threads: options.filter.list_threads(),
    };

    let sink = Arc::new(ResultSink::new(
        client.clone(),
        runtime.handle().clone(),
        handles,
    ));

    // Buck2 renders results itself, so nextest's own reporter is silenced;
    // `--test-executor-stderr=-` on the Buck2 side is how a person sees it.
    let exit_code = cx.run_with_sink(options.cli_args, {
        let sink = Arc::clone(&sink);
        move |event| sink.write_event(event)
    });

    // Drain results before reporting the exit code, so Buck2 has every result
    // in hand when it is told the run is over.
    let sink = Arc::into_inner(sink).expect("the run has finished, so no callback holds the sink");
    let drained = sink.finish();

    // `cargo-nextest` treats a run with no tests in it as an error, on the
    // grounds that the person asked for tests and got none. Buck2 chooses the
    // target set itself, so `buck2 test //:some-library` finding no tests is
    // routine rather than a mistake -- and Buck2 says "0 tests" plainly either
    // way.
    let exit_code = match exit_code? {
        NextestExitCode::NO_TESTS_RUN => 0,
        code => code,
    };
    drained?;

    let mut client = client;
    runtime
        .block_on(client.end_of_test_results(EndOfTestResultsRequest { exit_code }))
        .map_err(|status| ExpectedError::ReportResultsError {
            status: Box::new(status),
        })?;

    Ok(exit_code)
}
