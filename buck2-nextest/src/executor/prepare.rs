// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Turning Buck2's specs into targets nextest can run.
//!
//! A spec's command and environment are not directly runnable: Buck2 may send
//! opaque handles in place of values that are expensive or awkward to
//! serialise, and it does not say in the spec where the test should run. The
//! `PrepareForLocalExecution` RPC resolves all three -- it takes the spec's
//! values back, handles and all, and returns a full argument vector, a working
//! directory, and a verbatim environment.
//!
//! This is the reason the gRPC path supports handles where the file-based path
//! must reject them: resolving one means asking Buck2, and only this path has
//! anything to ask.

use crate::{
    errors::{ExpectedError, Result},
    proto::{
        ArgValue, ArgValueContent, ConfiguredTargetHandle, EnvironmentVariable, ExternalRunnerSpec,
        PrepareForLocalExecutionRequest, TestExecutable, TestStage, arg_value_content,
        test_orchestrator_client::TestOrchestratorClient, test_stage,
    },
    spec::{Buck2TestTarget, ConfiguredTarget, SUPPORTED_TEST_TYPE},
};
use std::collections::BTreeMap;
use tonic::transport::Channel;

/// A target ready to run, plus the handle Buck2 knows it by.
///
/// The handle is not part of [`Buck2TestTarget`] because it means nothing
/// outside this protocol; results are reported against it, so it is carried
/// alongside instead.
#[derive(Clone, Debug)]
pub(super) struct PreparedTarget {
    /// The target, in the same shape the file-based path produces.
    pub(super) target: Buck2TestTarget,

    /// The handle to report results against.
    pub(super) handle: ConfiguredTargetHandle,
}

/// Resolves every spec into a runnable target.
///
/// One round trip per target, in order. Buck2 has the answers in hand by this
/// point -- it built the targets before sending them -- so these are quick, and
/// issuing them in sequence keeps the failure that stops the run attributable
/// to the target that caused it.
pub(super) async fn prepare_all(
    client: &mut TestOrchestratorClient<Channel>,
    specs: Vec<ExternalRunnerSpec>,
) -> Result<Vec<PreparedTarget>> {
    let mut prepared = Vec::with_capacity(specs.len());
    for spec in specs {
        prepared.push(prepare_one(client, spec).await?);
    }
    Ok(prepared)
}

async fn prepare_one(
    client: &mut TestOrchestratorClient<Channel>,
    spec: ExternalRunnerSpec,
) -> Result<PreparedTarget> {
    let target = spec
        .target
        .clone()
        .ok_or_else(|| ExpectedError::SpecMissingField {
            field: "target",
            label: "<unknown>".to_owned(),
        })?;
    let configured = ConfiguredTarget {
        cell: target.cell.clone(),
        package: target.package.clone(),
        target: target.target.clone(),
        configuration: Some(target.configuration.clone()).filter(|c| !c.is_empty()),
    };
    let label = configured.label();

    if spec.test_type != SUPPORTED_TEST_TYPE {
        return Err(ExpectedError::UnsupportedTestType {
            label,
            test_type: spec.test_type,
        });
    }

    let handle = target
        .handle
        .ok_or_else(|| ExpectedError::SpecMissingField {
            field: "target.handle",
            label: label.clone(),
        })?;

    let request = PrepareForLocalExecutionRequest {
        test_executable: Some(TestExecutable {
            // Buck2 uses the stage for bookkeeping; nextest runs the same
            // binary to list and to execute, so it asks once, as a listing.
            stage: Some(TestStage {
                item: Some(test_stage::Item::Listing(test_stage::Listing {
                    suite: label.clone(),
                    cacheable: false,
                })),
            }),
            target: Some(handle),
            cmd: spec.command.into_iter().map(spec_arg).collect(),
            pre_create_dirs: Vec::new(),
            env: spec
                .env
                .into_iter()
                .map(|(key, value)| EnvironmentVariable {
                    key,
                    value: Some(spec_arg(value)),
                })
                .collect(),
        }),
        required_local_resources: Vec::new(),
    };

    let response = client
        .prepare_for_local_execution(request)
        .await
        .map_err(|status| ExpectedError::PrepareForLocalExecutionError {
            label: label.clone(),
            status: Box::new(status),
        })?
        .into_inner();

    let result = response
        .result
        .ok_or_else(|| ExpectedError::SpecMissingField {
            field: "PrepareForLocalExecutionResponse.result",
            label: label.clone(),
        })?;

    let mut cmd = result.cmd.into_iter();
    let program = cmd.next().ok_or_else(|| ExpectedError::EmptyCommand {
        label: label.clone(),
    })?;

    Ok(PreparedTarget {
        target: Buck2TestTarget {
            label,
            target: configured,
            program,
            leading_args: cmd.collect(),
            env: result
                .env
                .into_iter()
                .map(|var| (var.key, var.value))
                .collect::<BTreeMap<_, _>>(),
            // Buck2 is authoritative about where a test runs, so an empty
            // string means "it did not say" rather than "the filesystem root".
            cwd: Some(result.cwd)
                .filter(|cwd| !cwd.is_empty())
                .map(Into::into),
        },
        handle,
    })
}

/// Wraps a spec value as an argument, leaving handles for Buck2 to resolve.
fn spec_arg(value: crate::proto::ExternalRunnerSpecValue) -> ArgValue {
    ArgValue {
        content: Some(ArgValueContent {
            value: Some(arg_value_content::Value::SpecValue(value)),
        }),
        format: None,
    }
}
