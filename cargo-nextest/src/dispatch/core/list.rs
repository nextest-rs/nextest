// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! List command options and execution.

use super::{
    filter::TestBuildFilter,
    run::App,
    value_enums::{ListType, MessageFormatOpts},
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
    list::TestExecuteContext,
    pager::PagedOutput,
    run_mode::NextestRunMode,
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

    #[clap(flatten)]
    pub(crate) reuse_build: ReuseBuildOpts,
}

impl App {
    pub(crate) fn exec_list(
        &self,
        message_format: MessageFormatOpts,
        list_type: ListType,
    ) -> Result<()> {
        let pcx = ParseContext::new(self.base.graph());

        let (version_only_config, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self.base.load_profile(&config)?;
        let filter_exprs =
            build_filtersets(&pcx, &self.build_filter.filterset, FiltersetKind::Test)?;
        let test_filter_builder = self
            .build_filter
            .make_test_filter_builder(NextestRunMode::Test, filter_exprs)?;

        let binary_list = self.base.build_binary_list("test")?;

        let resolved_user_config = resolve_user_config(
            &self.base.build_platforms.host.platform,
            self.base.early_args.user_config_location(),
        )?;
        let (pager_setting, paginate) =
            self.base.early_args.resolve_pager(&resolved_user_config.ui);

        let should_page =
            !matches!(paginate, PaginateSetting::Never) && message_format.is_human_readable();

        let mut paged_output = if should_page {
            PagedOutput::request_pager(
                &pager_setting,
                paginate,
                &resolved_user_config.ui.streampager,
            )
        } else {
            PagedOutput::terminal()
        };

        let is_interactive = paged_output.is_interactive();
        let should_colorize = self
            .base
            .output
            .color
            .should_colorize(supports_color::Stream::Stdout);

        match list_type {
            ListType::BinariesOnly => {
                binary_list.write(
                    message_format.to_output_format(self.base.output.verbose, is_interactive),
                    &mut paged_output,
                    should_colorize,
                )?;
            }
            ListType::Full => {
                let double_spawn = self.base.load_double_spawn();
                let target_runner = self
                    .base
                    .load_runner(&binary_list.rust_build_meta.build_platforms);
                let profile =
                    profile.apply_build_platforms(&binary_list.rust_build_meta.build_platforms);
                let ctx = TestExecuteContext {
                    profile_name: profile.name(),
                    double_spawn,
                    target_runner,
                };

                let test_list =
                    self.build_test_list(&ctx, binary_list, &test_filter_builder, &profile)?;

                test_list.write(
                    message_format.to_output_format(self.base.output.verbose, is_interactive),
                    &mut paged_output,
                    should_colorize,
                )?;
            }
        }

        paged_output
            .write_str_flush()
            .map_err(WriteTestListError::Io)?;
        paged_output.finalize();

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
        let (_, config) = self.base.load_config(&pcx, &BTreeSet::new())?;
        let profile = self.base.load_profile(&config)?;

        // Validate test groups before doing any other work.
        let mode = if groups.is_empty() {
            ShowTestGroupsMode::All
        } else {
            let groups = ShowTestGroups::validate_groups(&profile, groups)?;
            ShowTestGroupsMode::Only(groups)
        };
        let settings = ShowTestGroupSettings { mode, show_default };

        let filter_exprs =
            build_filtersets(&pcx, &self.build_filter.filterset, FiltersetKind::Test)?;
        let test_filter_builder = self
            .build_filter
            .make_test_filter_builder(NextestRunMode::Test, filter_exprs)?;

        let binary_list = self.base.build_binary_list("test")?;
        let build_platforms = binary_list.rust_build_meta.build_platforms.clone();

        let double_spawn = self.base.load_double_spawn();
        let target_runner = self.base.load_runner(&build_platforms);
        let profile = profile.apply_build_platforms(&build_platforms);
        let ctx = TestExecuteContext {
            profile_name: profile.name(),
            double_spawn,
            target_runner,
        };

        let test_list = self.build_test_list(&ctx, binary_list, &test_filter_builder, &profile)?;

        let resolved_user_config = resolve_user_config(
            &self.base.build_platforms.host.platform,
            self.base.early_args.user_config_location(),
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
