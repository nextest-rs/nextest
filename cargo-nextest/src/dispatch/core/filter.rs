// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test filtering options for core commands.

use super::value_enums::{PlatformFilterOpts, RunIgnoredOpt};
use crate::{ExpectedError, Result, reuse_build::make_path_mapper};
use camino::Utf8PathBuf;
use clap::{ArgAction, Args};
use guppy::graph::PackageGraph;
use nextest_runner::{
    cargo_config::EnvironmentMap,
    config::core::{EvaluatableProfile, get_num_cpus},
    list::{BinaryList, RustTestArtifact, TestExecuteContext, TestList},
    partition::PartitionerBuilder,
    reuse_build::ReuseBuildInfo,
    run_mode::NextestRunMode,
    test_filter::{FilterBound, RunIgnored, TestFilterBuilder, TestFilterPatterns},
};
use std::sync::Arc;

/// Test filtering options.
#[derive(Debug, Args)]
#[command(next_help_heading = "Filter options")]
pub(crate) struct TestBuildFilter {
    /// Run ignored tests.
    #[arg(long, value_enum, value_name = "WHICH")]
    run_ignored: Option<RunIgnoredOpt>,

    /// Test partition, e.g. hash:1/2 or count:2/3.
    #[arg(long)]
    partition: Option<PartitionerBuilder>,

    /// Filter test binaries by build platform (DEPRECATED).
    ///
    /// Instead, use -E with 'platform(host)' or 'platform(target)'.
    #[arg(
        long,
        hide_short_help = true,
        value_enum,
        value_name = "PLATFORM",
        default_value_t
    )]
    pub(crate) platform_filter: PlatformFilterOpts,

    /// Test filterset (see {n}<https://nexte.st/docs/filtersets>).
    #[arg(
        long,
        alias = "filter-expr",
        short = 'E',
        value_name = "EXPR",
        action(ArgAction::Append)
    )]
    pub(crate) filterset: Vec<String>,

    /// Ignore the default filter configured in the profile.
    ///
    /// By default, all filtersets are intersected with the default filter configured in the
    /// profile. This flag disables that behavior.
    ///
    /// This flag doesn't change the definition of the `default()` filterset.
    #[arg(long)]
    ignore_default_filter: bool,

    /// Test name filters.
    #[arg(help_heading = None, name = "FILTERS")]
    pre_double_dash_filters: Vec<String>,

    /// Test name filters and emulated test binary arguments.
    ///
    /// Supported arguments:
    ///
    /// - --ignored:         Only run ignored tests
    /// - --include-ignored: Run both ignored and non-ignored tests
    /// - --skip PATTERN:    Skip tests that match the pattern
    /// - --exact:           Run tests that exactly match patterns after `--`
    #[arg(help_heading = None, value_name = "FILTERS_AND_ARGS", last = true)]
    filters: Vec<String>,
}

impl TestBuildFilter {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn compute_test_list<'g>(
        &self,
        ctx: &TestExecuteContext<'_>,
        graph: &'g PackageGraph,
        workspace_root: Utf8PathBuf,
        binary_list: Arc<BinaryList>,
        test_filter_builder: &TestFilterBuilder,
        env: EnvironmentMap,
        profile: &EvaluatableProfile<'_>,
        reuse_build: &ReuseBuildInfo,
    ) -> Result<TestList<'g>> {
        let path_mapper = make_path_mapper(
            reuse_build,
            graph,
            &binary_list.rust_build_meta.target_directory,
        )?;

        let rust_build_meta = binary_list.rust_build_meta.map_paths(&path_mapper);
        let test_artifacts = RustTestArtifact::from_binary_list(
            graph,
            binary_list,
            &rust_build_meta,
            &path_mapper,
            self.platform_filter.into(),
        )?;
        TestList::new(
            ctx,
            test_artifacts,
            rust_build_meta,
            test_filter_builder,
            workspace_root,
            env,
            profile,
            if self.ignore_default_filter {
                FilterBound::All
            } else {
                FilterBound::DefaultSet
            },
            // TODO: do we need to allow customizing this?
            get_num_cpus(),
        )
        .map_err(|err| ExpectedError::CreateTestListError { err })
    }

    pub(crate) fn make_test_filter_builder(
        &self,
        mode: NextestRunMode,
        filter_exprs: Vec<nextest_filtering::Filterset>,
    ) -> Result<TestFilterBuilder> {
        // Merge the test binary args into the patterns.
        let mut run_ignored = self.run_ignored.map(Into::into);
        let mut patterns = TestFilterPatterns::new(self.pre_double_dash_filters.clone());
        self.merge_test_binary_args(&mut run_ignored, &mut patterns)?;

        Ok(TestFilterBuilder::new(
            mode,
            run_ignored.unwrap_or_default(),
            self.partition.clone(),
            patterns,
            filter_exprs,
        )?)
    }

    fn merge_test_binary_args(
        &self,
        run_ignored: &mut Option<RunIgnored>,
        patterns: &mut TestFilterPatterns,
    ) -> Result<()> {
        // First scan to see if `--exact` is specified. If so, then everything here will be added to
        // `--exact`.
        let mut is_exact = false;
        for arg in &self.filters {
            if arg == "--" {
                break;
            }
            if arg == "--exact" {
                if is_exact {
                    return Err(ExpectedError::test_binary_args_parse_error(
                        "duplicated",
                        vec![arg.clone()],
                    ));
                }
                is_exact = true;
            }
        }

        let mut ignore_filters = Vec::new();
        let mut read_trailing_filters = false;

        let mut unsupported_args = Vec::new();

        let mut it = self.filters.iter();
        while let Some(arg) = it.next() {
            if read_trailing_filters || !arg.starts_with('-') {
                if is_exact {
                    patterns.add_exact_pattern(arg.clone());
                } else {
                    patterns.add_substring_pattern(arg.clone());
                }
            } else if arg == "--include-ignored" {
                ignore_filters.push((arg.clone(), RunIgnored::All));
            } else if arg == "--ignored" {
                ignore_filters.push((arg.clone(), RunIgnored::Only));
            } else if arg == "--" {
                read_trailing_filters = true;
            } else if arg == "--skip" {
                let skip_arg = it.next().ok_or_else(|| {
                    ExpectedError::test_binary_args_parse_error(
                        "missing required argument",
                        vec![arg.clone()],
                    )
                })?;

                if is_exact {
                    patterns.add_skip_exact_pattern(skip_arg.clone());
                } else {
                    patterns.add_skip_pattern(skip_arg.clone());
                }
            } else if arg == "--exact" {
                // Already handled above.
            } else {
                unsupported_args.push(arg.clone());
            }
        }

        for (s, f) in ignore_filters {
            if let Some(run_ignored) = run_ignored {
                if *run_ignored != f {
                    return Err(ExpectedError::test_binary_args_parse_error(
                        "mutually exclusive",
                        vec![s],
                    ));
                } else {
                    return Err(ExpectedError::test_binary_args_parse_error(
                        "duplicated",
                        vec![s],
                    ));
                }
            } else {
                *run_ignored = Some(f);
            }
        }

        if !unsupported_args.is_empty() {
            return Err(ExpectedError::test_binary_args_parse_error(
                "unsupported",
                unsupported_args,
            ));
        }

        Ok(())
    }
}

/// Archive build filtering options.
#[derive(Debug, Args)]
#[command(next_help_heading = "Filter options")]
pub(crate) struct ArchiveBuildFilter {
    /// Archive filterset (see <https://nexte.st/docs/filtersets>).
    ///
    /// This argument does not accept test predicates.
    #[arg(long, short = 'E', value_name = "EXPR", action(ArgAction::Append))]
    pub(crate) filterset: Vec<String>,
}
