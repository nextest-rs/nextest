// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Inherit settings for profiles
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Inherits(Option<String>);

impl Inherits {
    /// Creates a new `Inherits`.
    pub fn new(inherits: Option<String>) -> Self {
        Self(inherits)
    }

    /// Returns the profile that the custom profile inherits from
    pub fn inherits_from(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

// TODO: Need to write test cases  for this
#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::{
        config::{
            core::NextestConfig, overrides::DeserializedOverride,
            scripts::DeserializedProfileScriptConfig, utils::test_helpers::*,
        },
        errors::{
            ConfigParseErrorKind,
            InheritsError::{self, *},
        },
        reporter::{FinalStatusLevel, StatusLevel, TestOutputDisplay},
    };
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use nextest_filtering::ParseContext;
    use std::collections::HashSet;
    use test_case::test_case;

    #[derive(Default)]
    #[allow(dead_code)]
    pub struct CustomProfileTest {
        /// The default set of tests run by `cargo nextest run`.
        name: String,
        default_filter: Option<String>,
        retries: Option<RetryPolicy>,
        test_threads: Option<TestThreads>,
        threads_required: Option<ThreadsRequired>,
        run_extra_args: Option<Vec<String>>,
        status_level: Option<StatusLevel>,
        final_status_level: Option<FinalStatusLevel>,
        failure_output: Option<TestOutputDisplay>,
        success_output: Option<TestOutputDisplay>,
        max_fail: Option<MaxFail>,
        slow_timeout: Option<SlowTimeout>,
        global_timeout: Option<GlobalTimeout>,
        leak_timeout: Option<LeakTimeout>,
        overrides: Vec<DeserializedOverride>,
        scripts: Vec<DeserializedProfileScriptConfig>,
        junit: JunitImpl,
        archive: Option<ArchiveConfig>,
        inherits: Option<String>,
    }

    #[test_case(
        indoc! {r#"            
            [profile.prof_a]
            inherits = "prof_b"
            
            [profile.prof_b]
            inherits = "default"
            fail-fast = { max-fail = 4 }
        "#},
        Ok(CustomProfileTest {
            name: "prof_a".to_string(),
            inherits: Some("prof_b".to_string()),
            max_fail: Some(MaxFail::Count { max_fail: 4, terminate: TerminateMode::Wait }),
            ..Default::default()
        })
        ; "custom profile a inherits from another custom profile b"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            inherits = "prof_a"
            
            [profile.prof_a]
            inherits = "prof_b"
            
            [profile.prof_b]
            inherits = "prof_c"

            [profile.prof_c]
            inherits = "prof_a"

            [profile.prof_d]
            inherits = "prof_d"

            [profile.prof_e]
            inherits = "nonexistent_profile"
        "#},
        Err(
            vec![
                InheritsError::DefaultProfileInheritance("default".to_string()),
                InheritsError::SelfReferentialInheritance("prof_d".to_string()),
                InheritsError::UnknownInheritance("prof_e".to_string(), "nonexistent_profile".to_string()),
                InheritsError::InheritanceCycle(vec![vec!["prof_a".to_string(),"prof_b".to_string(), "prof_c".to_string()]]),
            ]
        )
        ; "inheritance errors detected"
    )]
    fn profile_inheritance(
        config_contents: &str,
        expected: Result<CustomProfileTest, Vec<InheritsError>>,
    ) {
        let workspace_dir = tempdir().unwrap();
        let graph = temp_workspace(&workspace_dir, config_contents);
        let pcx = ParseContext::new(&graph);

        let config_res = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            [],
            &Default::default(),
        );

        match expected {
            Ok(custom_profile) => {
                let config = config_res.expect("config is valid");
                let profile = config
                    .profile(&custom_profile.name)
                    .unwrap_or_else(|_| panic!("{} profile is known", &custom_profile.name));
                let profile = profile.apply_build_platforms(&build_platforms());
                assert_eq!(profile.inherits(), custom_profile.inherits.as_deref());
                assert_eq!(
                    profile.max_fail(),
                    custom_profile.max_fail.expect("max fail should exist")
                );
            }
            Err(expected_inherits_err) => {
                let error = config_res.expect_err("config is invalid");
                assert_eq!(error.tool(), None);
                match error.kind() {
                    ConfigParseErrorKind::InheritanceErrors(inherits_err) => {
                        // because inheritance errors are not in a deterministic order in the Vec<InheritsError>
                        // we use a Hashset here to test whether the error seen by the expected err
                        let expected_err: HashSet<&InheritsError> =
                            expected_inherits_err.iter().collect();
                        for actual_err in inherits_err.iter() {
                            match actual_err {
                                InheritanceCycle(sccs) => {
                                    // to check if the sccs exists in our expected errors,
                                    // we must sort the SCC (since these SCC chain are also
                                    // in a non-deterministic order). this runs under the
                                    // assumption that our expected_err contains the SCC cycle
                                    // in alphabetical sorting order as well
                                    let mut sccs = sccs.clone();
                                    for scc in sccs.iter_mut() {
                                        scc.sort()
                                    }
                                    assert!(
                                        expected_err.contains(&InheritanceCycle(sccs)),
                                        "unexpected inherit error {:?}",
                                        actual_err
                                    )
                                }
                                _ => {
                                    assert!(
                                        expected_err.contains(&actual_err),
                                        "unexpected inherit error {:?}",
                                        actual_err
                                    )
                                }
                            }
                        }
                    }
                    other => {
                        panic!("expected ConfigParseErrorKind::InheritanceErrors, got {other}")
                    }
                }
            }
        }
    }
}
