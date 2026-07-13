// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! List command options and execution.

use super::{
    filter::TestBuildFilter,
    run::App,
    value_enums::{ListType, MessageFormatOpts, ShowProgressOpt},
};
use crate::{
    Result,
    cargo_cli::CargoOptions,
    dispatch::helpers::{build_filtersets, resolve_user_config},
    reuse_build::ReuseBuildOpts,
};
use clap::Args;
use nextest_filtering::{FiltersetKind, ParseContext};
use nextest_runner::{
    errors::WriteTestListError,
    helpers::force_or_new_run_id,
    list::TestExecuteContext,
    pager::PagedOutput,
    reporter::ShowProgress,
    run_mode::NextestRunMode,
    runner::VersionEnvVars,
    show_config::{ShowTestGroupSettings, ShowTestGroups, ShowTestGroupsMode},
    user_config::elements::PaginateSetting,
    write_str::WriteStr,
};
use std::collections::BTreeSet;

/// Options for the list command.
#[derive(Debug, Args)]
pub(crate) struct ListOpts {
    #[clap(flatten)]
    pub(crate) cargo_options: CargoOptions,

    #[clap(flatten)]
    pub(crate) build_filter: TestBuildFilter,

    /// Output format.
    #[arg(
        short = 'T',
        long,
        value_enum,
        default_value_t,
        help_heading = "Output options",
        value_name = "FMT"
    )]
    pub(crate) message_format: MessageFormatOpts,

    /// Type of listing.
    #[arg(
        long,
        value_enum,
        default_value_t,
        help_heading = "Output options",
        value_name = "TYPE"
    )]
    pub(crate) list_type: ListType,

    /// Show list progress in the specified manner.
    ///
    /// List progress is only shown if building the test list takes more than 2
    /// seconds.
    ///
    /// This can also be set via user config at `~/.config/nextest/config.toml`.
    /// See <https://nexte.st/docs/user-config>.
    #[arg(long, env = "NEXTEST_SHOW_PROGRESS", help_heading = "Output options")]
    pub(crate) show_progress: Option<ShowProgressOpt>,

    #[clap(flatten)]
    pub(crate) reuse_build: ReuseBuildOpts,
}

impl App {
    pub(crate) fn exec_list(
        &self,
        message_format: MessageFormatOpts,
        list_type: ListType,
        show_progress: Option<ShowProgressOpt>,
    ) -> Result<()> {
        let pcx = ParseContext::new(self.base.graph());

        let (version_only_config, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self.base.load_profile(&config)?;
        let known_groups = profile.known_groups();
        let filter_exprs = build_filtersets(
            &pcx,
            &self.build_filter.filterset,
            FiltersetKind::Test,
            &known_groups,
        )?;
        // no_capture is ignored for list commands.
        let test_filter = self
            .build_filter
            .make_test_filter(NextestRunMode::Test, filter_exprs)?
            .test_filter;

        let binary_list = self.base.build_binary_list("test")?;

        let resolved_user_config =
            resolve_user_config(self.base.early_args.user_config_location())?;
        let (pager_setting, paginate) =
            self.base.early_args.resolve_pager(&resolved_user_config.ui);

        let should_page =
            !matches!(paginate, PaginateSetting::Never) && message_format.is_human_readable();

        let should_colorize = self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stdout);

        match list_type {
            ListType::BinariesOnly => {
                let mut paged_output = if should_page {
                    PagedOutput::request_pager(
                        &pager_setting,
                        paginate,
                        &resolved_user_config.ui.streampager,
                    )
                } else {
                    PagedOutput::terminal()
                };
                let output_format = message_format
                    .to_output_format(self.base.output.verbose, paged_output.is_interactive());
                binary_list.write(output_format, &mut paged_output, should_colorize)?;
                paged_output
                    .write_str_flush()
                    .map_err(WriteTestListError::Io)?;
                paged_output.finalize();
            }
            ListType::Full => {
                let double_spawn = self.base.load_double_spawn();
                let target_runner = self
                    .base
                    .load_runner(&binary_list.rust_build_meta.build_platforms);
                let profile =
                    profile.apply_build_platforms(&binary_list.rust_build_meta.build_platforms);
                let nextest_version_config = version_only_config.nextest_version();
                let version_env_vars = VersionEnvVars {
                    current_version: self.base.current_version.clone(),
                    required_version: nextest_version_config.required.version().cloned(),
                    recommended_version: nextest_version_config.recommended.version().cloned(),
                };
                let ctx = TestExecuteContext {
                    run_id: force_or_new_run_id(),
                    version_env_vars: &version_env_vars,
                    profile_name: profile.name(),
                    double_spawn,
                    target_runner,
                };

                // The precedence for showing progress during listing is CLI ->
                // env -> resolved config, same as the run path.
                let list_show_progress = show_progress
                    .map(ShowProgress::from)
                    .unwrap_or_else(|| resolved_user_config.ui.show_progress.into());
                let test_list = self.build_test_list(
                    &ctx,
                    binary_list,
                    &test_filter,
                    &profile,
                    list_show_progress,
                )?;

                // Spawn the pager after building the list. (We sometimes
                // display a progress bar during the list phase -- we want to
                // ensure the pager doesn't collide with the progress bar.)
                let mut paged_output = if should_page {
                    PagedOutput::request_pager(
                        &pager_setting,
                        paginate,
                        &resolved_user_config.ui.streampager,
                    )
                } else {
                    PagedOutput::terminal()
                };
                let output_format = message_format
                    .to_output_format(self.base.output.verbose, paged_output.is_interactive());
                test_list.write(output_format, &mut paged_output, should_colorize)?;
                paged_output
                    .write_str_flush()
                    .map_err(WriteTestListError::Io)?;
                paged_output.finalize();
            }
        }

        self.base
            .check_version_config_final(version_only_config.nextest_version())?;
        Ok(())
    }

    pub(crate) fn exec_show_test_groups(
        &self,
        show_default: bool,
        groups: Vec<nextest_runner::config::elements::TestGroup>,
    ) -> Result<()> {
        let pcx = ParseContext::new(self.base.graph());
        let (version_only_config, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self.base.load_profile(&config)?;

        // Validate test groups before doing any other work.
        let mode = if groups.is_empty() {
            ShowTestGroupsMode::All
        } else {
            let groups = ShowTestGroups::validate_groups(&profile, groups)?;
            ShowTestGroupsMode::Only(groups)
        };
        let settings = ShowTestGroupSettings { mode, show_default };

        let known_groups = profile.known_groups();
        let filter_exprs = build_filtersets(
            &pcx,
            &self.build_filter.filterset,
            FiltersetKind::Test,
            &known_groups,
        )?;
        // no_capture is ignored for list commands.
        let test_filter = self
            .build_filter
            .make_test_filter(NextestRunMode::Test, filter_exprs)?
            .test_filter;

        let binary_list = self.base.build_binary_list("test")?;
        let build_platforms = binary_list.rust_build_meta.build_platforms.clone();

        let double_spawn = self.base.load_double_spawn();
        let target_runner = self.base.load_runner(&build_platforms);
        let profile = profile.apply_build_platforms(&build_platforms);
        let nextest_version_config = version_only_config.nextest_version();
        let version_env_vars = VersionEnvVars {
            current_version: self.base.current_version.clone(),
            required_version: nextest_version_config.required.version().cloned(),
            recommended_version: nextest_version_config.recommended.version().cloned(),
        };
        let ctx = TestExecuteContext {
            run_id: force_or_new_run_id(),
            version_env_vars: &version_env_vars,
            profile_name: profile.name(),
            double_spawn,
            target_runner,
        };

        let resolved_user_config =
            resolve_user_config(self.base.early_args.user_config_location())?;

        let test_list = self.build_test_list(
            &ctx,
            binary_list,
            &test_filter,
            &profile,
            resolved_user_config.ui.show_progress.into(),
        )?;

        let (pager_setting, paginate) =
            self.base.early_args.resolve_pager(&resolved_user_config.ui);

        let mut paged_output = PagedOutput::request_pager(
            &pager_setting,
            paginate,
            &resolved_user_config.ui.streampager,
        );

        let show_test_groups = ShowTestGroups::new(&profile, &test_list, &settings);
        show_test_groups
            .write_human(
                &mut paged_output,
                self.base
                    .output
                    .color
                    .should_colorize(supports_color::Stream::Stdout),
            )
            .map_err(WriteTestListError::Io)?;

        paged_output
            .write_str_flush()
            .map_err(WriteTestListError::Io)?;
        paged_output.finalize();

        Ok(())
    }
}
