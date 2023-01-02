// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    config::{
        CustomTestGroup, FinalConfig, NextestProfile, OverrideId, PreBuildPlatform,
        ProfileOverrideImpl, TestGroup, TestGroupConfig,
    },
    errors::ShowTestGroupsError,
    helpers::QuotedDisplay,
    list::{TestInstance, TestList, TestListDisplayFilter},
};
use indexmap::IndexMap;
use owo_colors::{OwoColorize, Style};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
};
use target_spec::TargetSpec;

/// Shows sets of tests that are in various groups.
#[derive(Debug)]
pub struct ShowTestGroups<'a> {
    test_list: &'a TestList<'a>,
    indexed_overrides: BTreeMap<TestGroup, IndexMap<OverrideId, ShowTestGroupsData<'a>>>,
    test_group_config: &'a BTreeMap<CustomTestGroup, TestGroupConfig>,
    // This is Some iff settings.show_default is true.
    non_overrides: Option<TestListDisplayFilter<'a>>,
}

impl<'a> ShowTestGroups<'a> {
    /// Validates that the given groups are known to this profile.
    pub fn validate_groups(
        profile: &NextestProfile<'_, PreBuildPlatform>,
        groups: impl IntoIterator<Item = TestGroup>,
    ) -> Result<ValidatedTestGroups, ShowTestGroupsError> {
        let groups = groups.into_iter().collect();
        let known_groups: BTreeSet<_> =
            TestGroup::make_all_groups(profile.test_group_config().keys().cloned()).collect();
        let unknown_groups = &groups - &known_groups;
        if !unknown_groups.is_empty() {
            return Err(ShowTestGroupsError::UnknownGroups {
                unknown_groups,
                known_groups,
            });
        }
        Ok(ValidatedTestGroups(groups))
    }

    /// Creates a new `ShowTestGroups` from the given profile and test list.
    pub fn new(
        profile: &'a NextestProfile<'a>,
        test_list: &'a TestList<'a>,
        settings: &ShowTestGroupSettings,
    ) -> Self {
        let mut indexed_overrides: BTreeMap<_, _> =
            TestGroup::make_all_groups(profile.test_group_config().keys().cloned())
                .filter_map(|group| {
                    settings
                        .mode
                        .matches_group(&group)
                        .then(|| (group, IndexMap::new()))
                })
                .collect();
        let mut non_overrides = settings.show_default.then(TestListDisplayFilter::new);

        for suite in test_list.iter() {
            for (test_name, test_case) in suite.status.test_cases() {
                let test_instance = TestInstance::new(test_name, suite, test_case);
                let query = test_instance.to_test_query();
                let profile_overrides = profile.overrides_with_source_for(&query);

                if let Some((test_group, source)) = profile_overrides.test_group_with_source() {
                    let override_map = match indexed_overrides.get_mut(test_group) {
                        Some(override_map) => override_map,
                        None => continue,
                    };
                    let data = override_map
                        .entry(source.id().clone())
                        .or_insert_with(|| ShowTestGroupsData::new(source));
                    data.matching_tests.insert(&suite.binary_id, test_name);
                } else if let Some(non_overrides) = non_overrides.as_mut() {
                    if settings.mode.matches_group(&TestGroup::Global) {
                        non_overrides.insert(&suite.binary_id, test_name);
                    }
                }
            }
        }

        Self {
            test_list,
            indexed_overrides,
            test_group_config: profile.test_group_config(),
            non_overrides,
        }
    }

    fn should_show_group(&self, group: &TestGroup) -> bool {
        // So this is a bit tricky. We want to show a group if it matches the filter.
        //
        //     group     filter    show-default   |   show?
        //    -------   --------   -------------  |  -------
        //    @global    matches       true       |   always
        //    @global    matches      false       |   only if any overrides set @global
        //    @global   no match         *        |   false  [1]
        //     custom    matches         *        |   always
        //     custom   no match         *        |   false  [1]
        //
        // [1]: filtered out by the constructor above, so not handled below

        match (group, self.non_overrides.is_some()) {
            (TestGroup::Global, true) => true,
            (TestGroup::Global, false) => self
                .indexed_overrides
                .get(group)
                .map(|override_map| !override_map.values().all(|data| data.is_empty()))
                .unwrap_or(false),
            _ => true,
        }
    }

    /// Writes the test groups to the given writer in a human-friendly format.
    pub fn write_human(&self, mut writer: &mut dyn Write, colorize: bool) -> io::Result<()> {
        static INDENT: &str = "      ";

        let mut styles = Styles::default();
        if colorize {
            styles.colorize();
        }

        for (test_group, override_map) in &self.indexed_overrides {
            if !self.should_show_group(test_group) {
                continue;
            }

            write!(writer, "group: {}", test_group.style(styles.group))?;
            if let TestGroup::Custom(group) = test_group {
                write!(
                    writer,
                    " (max threads = {})",
                    self.test_group_config[group]
                        .max_threads
                        .style(styles.max_threads)
                )?;
            }
            writeln!(writer)?;

            let mut any_printed = false;

            for (override_id, data) in override_map {
                any_printed = true;
                write!(
                    writer,
                    "  * override for {} profile",
                    override_id.profile_name.style(styles.profile),
                )?;

                if let Some((filter_str, _)) = data.override_.filter() {
                    write!(
                        writer,
                        " with filter {}",
                        QuotedDisplay(filter_str).style(styles.filter)
                    )?;
                }
                if let Some(target_spec) = data.override_.target_spec() {
                    // TODO: add fmt::Display impl for target_spec.
                    let platform_str = match target_spec {
                        TargetSpec::Triple(triple) => triple.as_str(),
                        TargetSpec::Expression(expr) => expr.expression_str(),
                    };

                    write!(
                        writer,
                        " on platform {}",
                        QuotedDisplay(platform_str).style(styles.platform)
                    )?;
                }

                writeln!(writer, ":")?;

                let mut inner_writer = indent_write::io::IndentWriter::new(INDENT, writer);
                self.test_list.write_human_with_filter(
                    &data.matching_tests,
                    &mut inner_writer,
                    false,
                    colorize,
                )?;
                inner_writer.flush()?;
                writer = inner_writer.into_inner();
            }

            // Also show tests that don't match an override if they match the global config below.
            if test_group == &TestGroup::Global {
                if let Some(non_overrides) = &self.non_overrides {
                    any_printed = true;
                    writeln!(writer, "  * from default settings:")?;
                    let mut inner_writer = indent_write::io::IndentWriter::new(INDENT, writer);
                    self.test_list.write_human_with_filter(
                        non_overrides,
                        &mut inner_writer,
                        false,
                        colorize,
                    )?;
                    inner_writer.flush()?;
                    writer = inner_writer.into_inner();
                }
            }

            if !any_printed {
                writeln!(writer, "    (no matches)")?;
            }
        }

        Ok(())
    }
}

/// Settings for showing test groups.
#[derive(Clone, Debug)]
pub struct ShowTestGroupSettings {
    /// Whether to show tests that have default settings and don't match any overrides.
    pub show_default: bool,

    /// Which groups of tests to show.
    pub mode: ShowTestGroupsMode,
}

/// Which groups of tests to show.
#[derive(Clone, Debug)]
pub enum ShowTestGroupsMode {
    /// Show all groups.
    All,
    /// Show only the named groups.
    Only(ValidatedTestGroups),
}

impl ShowTestGroupsMode {
    fn matches_group(&self, group: &TestGroup) -> bool {
        match self {
            Self::All => true,
            Self::Only(groups) => groups.0.contains(group),
        }
    }
}

/// Validated test groups, part of [`ShowTestGroupsMode`].
#[derive(Clone, Debug)]
pub struct ValidatedTestGroups(BTreeSet<TestGroup>);

impl ValidatedTestGroups {
    /// Returns the set of test groups.
    pub fn into_inner(self) -> BTreeSet<TestGroup> {
        self.0
    }
}

#[derive(Debug)]
struct ShowTestGroupsData<'a> {
    override_: &'a ProfileOverrideImpl<FinalConfig>,
    matching_tests: TestListDisplayFilter<'a>,
}

impl<'a> ShowTestGroupsData<'a> {
    fn new(override_: &'a ProfileOverrideImpl<FinalConfig>) -> Self {
        Self {
            override_,
            matching_tests: TestListDisplayFilter::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.matching_tests.test_count() == 0
    }
}

#[derive(Clone, Debug, Default)]
struct Styles {
    group: Style,
    max_threads: Style,
    profile: Style,
    filter: Style,
    platform: Style,
}

impl Styles {
    fn colorize(&mut self) {
        self.group = Style::new().bold().underline();
        self.max_threads = Style::new().bold();
        self.profile = Style::new().bold();
        self.filter = Style::new().yellow();
        self.platform = Style::new().yellow();
    }
}
