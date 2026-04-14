// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Precomputation of test group memberships for `group()` filterset
//! evaluation.
//!
//! Group membership is determined entirely by config overrides that set
//! `test-group`. Since `group()` is banned in override filters, the
//! membership is a pure function of the config and can be computed in a
//! single pass.

use crate::{
    config::elements::TestGroup,
    list::{OwnedTestInstanceId, TestInstanceId, TestInstanceIdKey},
};
use nextest_filtering::{GroupLookup, NameMatcher, TestQuery};
use std::{collections::HashMap, fmt};

/// Precomputed test group memberships.
///
/// Built by [`EvaluatableProfile::precompute_group_memberships`] and
/// implements [`GroupLookup`] to resolve `group()` predicates in CLI
/// filtersets.
///
/// [`EvaluatableProfile::precompute_group_memberships`]: crate::config::EvaluatableProfile::precompute_group_memberships
pub struct PrecomputedGroupMembership {
    /// Map from test identity to the assigned test group. Tests not
    /// present in this map are implicitly in `@global`.
    assignments: HashMap<OwnedTestInstanceId, TestGroup>,
}

impl PrecomputedGroupMembership {
    /// Creates an empty membership with no custom group assignments.
    pub fn empty() -> Self {
        Self {
            assignments: HashMap::new(),
        }
    }

    /// Inserts a group assignment for the given test. Only custom
    /// (non-`@global`) groups need to be inserted; `@global` is the
    /// implicit default.
    pub(crate) fn insert(&mut self, id: OwnedTestInstanceId, group: TestGroup) {
        self.assignments.insert(id, group);
    }
}

impl fmt::Debug for PrecomputedGroupMembership {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrecomputedGroupMembership")
            .field("num_assignments", &self.assignments.len())
            .finish()
    }
}

impl GroupLookup for PrecomputedGroupMembership {
    fn is_member_test(&self, test: &TestQuery<'_>, matcher: &NameMatcher) -> bool {
        let key = TestInstanceId {
            binary_id: test.binary_query.binary_id,
            test_name: test.test_name,
        };
        // Look up via the borrow-complex-key pattern, avoiding a clone.
        match self.assignments.get(&key as &dyn TestInstanceIdKey) {
            Some(TestGroup::Custom(group)) => matcher.is_match(group.as_str()),
            Some(TestGroup::Global) | None => matcher.is_match(nextest_metadata::GLOBAL_TEST_GROUP),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::{
            core::{EvaluatableProfile, NextestConfig},
            elements::TestGroup,
            overrides::TestSettings,
            utils::test_helpers::*,
        },
        run_mode::NextestRunMode,
    };
    use camino_tempfile::tempdir;
    use guppy::graph::{PackageGraph, cargo::BuildPlatform};
    use indoc::indoc;
    use nextest_filtering::{
        EvalContext, Filterset, FiltersetKind, KnownGroups, ParseContext, TestQuery,
    };
    use nextest_metadata::TestCaseName;
    use std::collections::HashSet;

    /// Parses a config and returns the evaluatable profile along with
    /// the graph and package ID needed for constructing queries.
    fn setup_profile(config_contents: &str) -> (EvaluatableProfile<'static>, SetupContext) {
        let workspace_dir = tempdir().unwrap();
        let graph = temp_workspace(&workspace_dir, config_contents);
        let package_id = graph.workspace().iter().next().unwrap().id().clone();
        let pcx = ParseContext::new(&graph);
        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &[][..],
            &Default::default(),
        )
        .expect("config is valid");
        // Leak to avoid lifetime issues in tests.
        let config = Box::leak(Box::new(config));
        let graph = Box::leak(Box::new(graph));
        let profile = config
            .profile("default")
            .expect("default profile")
            .apply_build_platforms(&build_platforms());
        (profile, SetupContext { graph, package_id })
    }

    struct SetupContext {
        graph: &'static PackageGraph,
        package_id: guppy::PackageId,
    }

    /// Precomputes group membership for a single test and returns its
    /// settings.
    fn settings_for_test<'a>(
        profile: &'a EvaluatableProfile<'_>,
        cx: &'a SetupContext,
        test_name: &'a str,
    ) -> TestSettings<'a> {
        let bq = binary_query(
            cx.graph,
            &cx.package_id,
            "lib",
            "my-binary",
            BuildPlatform::Target,
        );
        let test_name = TestCaseName::new(test_name);
        let query = TestQuery {
            binary_query: bq.to_query(),
            test_name: &test_name,
        };
        profile.settings_for(NextestRunMode::Test, &query)
    }

    #[test]
    fn base_override_assigns_group() {
        let (profile, cx) = setup_profile(indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(my_test)"
            test-group = "serial"

            [test-groups.serial]
            max-threads = 1
        "#});
        let settings = settings_for_test(&profile, &cx, "my_test");
        assert_eq!(settings.test_group(), &test_group("serial"));
    }

    #[test]
    fn non_matching_test_stays_global() {
        let (profile, cx) = setup_profile(indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(my_test)"
            test-group = "serial"

            [test-groups.serial]
            max-threads = 1
        "#});
        let settings = settings_for_test(&profile, &cx, "other_test");
        assert_eq!(settings.test_group(), &TestGroup::Global);
    }

    #[test]
    fn first_override_wins() {
        let (profile, cx) = setup_profile(indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(my_test)"
            test-group = "serial"

            [[profile.default.overrides]]
            filter = "test(my_test)"
            test-group = "batch"

            [test-groups.serial]
            max-threads = 1

            [test-groups.batch]
            max-threads = 4
        "#});
        let settings = settings_for_test(&profile, &cx, "my_test");
        assert_eq!(settings.test_group(), &test_group("serial"));
    }

    #[test]
    fn group_lookup_resolves_membership() {
        // This test exercises the full pipeline: precompute group
        // memberships from config overrides, then use the result as
        // a GroupLookup to evaluate a group() filterset.
        let (profile, cx) = setup_profile(indoc! {r#"
            [[profile.default.overrides]]
            filter = "test(my_test)"
            test-group = "serial"

            [test-groups.serial]
            max-threads = 1
        "#});

        let bq = binary_query(
            cx.graph,
            &cx.package_id,
            "lib",
            "my-binary",
            BuildPlatform::Target,
        );
        let test_name = TestCaseName::new("my_test");
        let query = TestQuery {
            binary_query: bq.to_query(),
            test_name: &test_name,
        };

        // Precompute group membership.
        let membership = profile.precompute_group_memberships(std::iter::once(query));

        // Parse a group() filterset and evaluate with the membership.
        let pcx = ParseContext::new(cx.graph);
        let filterset = Filterset::parse(
            "group(serial)".to_owned(),
            &pcx,
            FiltersetKind::Test,
            &KnownGroups::Known {
                custom_groups: HashSet::from(["serial".to_owned()]),
            },
        )
        .expect("group(serial) parses");

        let ecx = EvalContext {
            default_filter: &profile.default_filter().expr,
        };

        // my_test is in the serial group, so group(serial) should match.
        assert!(
            filterset.matches_test_with_groups(&query, &ecx, &membership),
            "group(serial) should match my_test (in serial group)"
        );

        // other_test is not in any custom group.
        let other_name = TestCaseName::new("other_test");
        let other_query = TestQuery {
            binary_query: bq.to_query(),
            test_name: &other_name,
        };
        assert!(
            !filterset.matches_test_with_groups(&other_query, &ecx, &membership),
            "group(serial) should not match other_test (not in serial group)"
        );
    }
}
