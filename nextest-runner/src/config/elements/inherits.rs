// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Inherit settings for profiles.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Inherits(Option<String>);

impl Inherits {
    /// Creates a new `Inherits`.
    pub fn new(inherits: Option<String>) -> Self {
        Self(inherits)
    }

    /// Returns the profile that the custom profile inherits from.
    pub fn inherits_from(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::{
            core::{NextestConfig, ToolConfigFile, ToolName},
            elements::{MaxFail, RetryPolicy, TerminateMode},
            utils::test_helpers::*,
        },
        errors::{
            ConfigParseErrorKind,
            InheritsError::{self, *},
        },
    };
    use camino_tempfile::tempdir;
    use indoc::indoc;
    use nextest_filtering::ParseContext;
    use std::{collections::HashSet, fs};
    use test_case::test_case;

    fn tool_name(s: &str) -> ToolName {
        ToolName::new(s.into()).unwrap()
    }

    /// Settings checked for inheritance below.
    #[derive(Default)]
    #[allow(dead_code)]
    pub struct InheritSettings {
        name: String,
        inherits: Option<String>,
        max_fail: Option<MaxFail>,
        retries: Option<RetryPolicy>,
    }

    #[test_case(
        indoc! {r#"
            [profile.prof_a]
            inherits = "prof_b"

            [profile.prof_b]
            inherits = "prof_c"
            fail-fast = { max-fail = 4 }

            [profile.prof_c]
            inherits = "default"
            fail-fast = { max-fail = 10 }
            retries = 3
        "#},
        Ok(InheritSettings {
            name: "prof_a".to_string(),
            inherits: Some("prof_b".to_string()),
            // prof_b's max-fail (4) should override prof_c's (10)
            max_fail: Some(MaxFail::Count { max_fail: 4, terminate: TerminateMode::Wait }),
            // prof_c's retries should be inherited through prof_b
            retries: Some(RetryPolicy::new_without_delay(3)),
        })
        ; "three-level inheritance"
    )]
    #[test_case(
        indoc! {r#"
            [profile.prof_a]
            inherits = "prof_b"

            [profile.prof_b]
            inherits = "prof_c"

            [profile.prof_c]
            inherits = "prof_c"
        "#},
        Err(
            vec![
                InheritsError::SelfReferentialInheritance("prof_c".to_string()),
            ]
        ) ; "self referential error not inheritance cycle"
    )]
    #[test_case(
        indoc! {r#"
            [profile.prof_a]
            inherits = "prof_b"

            [profile.prof_b]
            inherits = "prof_c"

            [profile.prof_c]
            inherits = "prof_d"

            [profile.prof_d]
            inherits = "prof_e"

            [profile.prof_e]
            inherits = "prof_c"
        "#},
        Err(
            vec![
                InheritsError::InheritanceCycle(
                    vec![vec!["prof_c".to_string(),"prof_d".to_string(), "prof_e".to_string()]],
                ),
            ]
        ) ; "C to D to E SCC cycle"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default]
            inherits = "prof_a"

            [profile.default-miri]
            inherits = "prof_c"

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
                InheritsError::DefaultProfileInheritance("default-miri".to_string()),
                InheritsError::SelfReferentialInheritance("prof_d".to_string()),
                InheritsError::UnknownInheritance(
                    "prof_e".to_string(),
                    "nonexistent_profile".to_string(),
                ),
                InheritsError::InheritanceCycle(
                    vec![
                        vec!["prof_a".to_string(),"prof_b".to_string(), "prof_c".to_string()],
                    ]
                ),
            ]
        )
        ; "inheritance errors detected"
    )]
    #[test_case(
        indoc! {r#"
            [profile.my-profile]
            inherits = "default-nonexistent"
            retries = 5
        "#},
        Err(
            vec![
                InheritsError::UnknownInheritance(
                    "my-profile".to_string(),
                    "default-nonexistent".to_string(),
                ),
            ]
        )
        ; "inherit from nonexistent default profile"
    )]
    #[test_case(
        indoc! {r#"
            [profile.default-custom]
            retries = 3

            [profile.my-profile]
            inherits = "default-custom"
            fail-fast = { max-fail = 5 }
        "#},
        Ok(InheritSettings {
            name: "my-profile".to_string(),
            inherits: Some("default-custom".to_string()),
            max_fail: Some(MaxFail::Count { max_fail: 5, terminate: TerminateMode::Wait }),
            retries: Some(RetryPolicy::new_without_delay(3)),
        })
        ; "inherit from defined default profile"
    )]
    fn profile_inheritance(
        config_contents: &str,
        expected: Result<InheritSettings, Vec<InheritsError>>,
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
                let default_profile = config
                    .profile("default")
                    .unwrap_or_else(|_| panic!("default profile is known"));
                let default_profile = default_profile.apply_build_platforms(&build_platforms());
                let profile = config
                    .profile(&custom_profile.name)
                    .unwrap_or_else(|_| panic!("{} profile is known", &custom_profile.name));
                let profile = profile.apply_build_platforms(&build_platforms());
                assert_eq!(default_profile.inherits(), None);
                assert_eq!(profile.inherits(), custom_profile.inherits.as_deref());

                // Spot check that inheritance works correctly.
                assert_eq!(
                    profile.max_fail(),
                    custom_profile.max_fail.expect("max fail should exist")
                );
                if let Some(expected_retries) = custom_profile.retries {
                    assert_eq!(profile.retries(), expected_retries);
                }
            }
            Err(expected_inherits_err) => {
                let error = config_res.expect_err("config is invalid");
                assert_eq!(error.tool(), None);
                match error.kind() {
                    ConfigParseErrorKind::InheritanceErrors(inherits_err) => {
                        // Because inheritance errors are not in a deterministic
                        // order in the Vec<InheritsError>, we use a HashSet
                        // here to test whether the error seen by the expected
                        // err.
                        let expected_err: HashSet<&InheritsError> =
                            expected_inherits_err.iter().collect();
                        for actual_err in inherits_err.iter() {
                            match actual_err {
                                InheritanceCycle(sccs) => {
                                    // SCC vectors do show the cycle, but
                                    // we can't deterministically represent the cycle
                                    // (i.e. A->B->C->A could be {A,B,C}, {C,A,B}, or
                                    // {B,C,A})
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

    /// Test that higher-priority files can inherit from lower-priority files.
    #[test]
    fn valid_downward_inheritance() {
        let workspace_dir = tempdir().unwrap();

        // Tool config 1 (higher priority): defines prof_a inheriting from prof_b
        let tool1_config = workspace_dir.path().join("tool1.toml");
        fs::write(
            &tool1_config,
            indoc! {r#"
                    [profile.prof_a]
                    inherits = "prof_b"
                    retries = 5
                "#},
        )
        .unwrap();

        // Tool config 2 (lower priority): defines prof_b
        let tool2_config = workspace_dir.path().join("tool2.toml");
        fs::write(
            &tool2_config,
            indoc! {r#"
                    [profile.prof_b]
                    retries = 3
                "#},
        )
        .unwrap();

        let workspace_config = indoc! {r#"
                [profile.default]
            "#};

        let graph = temp_workspace(&workspace_dir, workspace_config);
        let pcx = ParseContext::new(&graph);

        // tool1 is first = higher priority, tool2 is second = lower priority
        let tool_configs = [
            ToolConfigFile {
                tool: tool_name("tool1"),
                config_file: tool1_config,
            },
            ToolConfigFile {
                tool: tool_name("tool2"),
                config_file: tool2_config,
            },
        ];

        let config = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &tool_configs,
            &Default::default(),
        )
        .expect("config should be valid");

        // prof_a should inherit retries=3 from prof_b, but override with retries=5
        let profile = config
            .profile("prof_a")
            .unwrap()
            .apply_build_platforms(&build_platforms());
        assert_eq!(profile.retries(), RetryPolicy::new_without_delay(5));

        // prof_b should have retries=3
        let profile = config
            .profile("prof_b")
            .unwrap()
            .apply_build_platforms(&build_platforms());
        assert_eq!(profile.retries(), RetryPolicy::new_without_delay(3));
    }

    /// Test that lower-priority files cannot inherit from higher-priority files.
    /// This is reported as an unknown profile error.
    #[test]
    fn invalid_upward_inheritance() {
        let workspace_dir = tempdir().unwrap();

        // Tool config 1 (higher priority): defines prof_a
        let tool1_config = workspace_dir.path().join("tool1.toml");
        fs::write(
            &tool1_config,
            indoc! {r#"
                    [profile.prof_a]
                    retries = 5
                "#},
        )
        .unwrap();

        // Tool config 2 (lower priority): tries to inherit from prof_a (not yet loaded)
        let tool2_config = workspace_dir.path().join("tool2.toml");
        fs::write(
            &tool2_config,
            indoc! {r#"
                    [profile.prof_b]
                    inherits = "prof_a"
                "#},
        )
        .unwrap();

        let workspace_config = indoc! {r#"
                [profile.default]
            "#};

        let graph = temp_workspace(&workspace_dir, workspace_config);
        let pcx = ParseContext::new(&graph);

        let tool_configs = [
            ToolConfigFile {
                tool: tool_name("tool1"),
                config_file: tool1_config,
            },
            ToolConfigFile {
                tool: tool_name("tool2"),
                config_file: tool2_config,
            },
        ];

        let error = NextestConfig::from_sources(
            graph.workspace().root(),
            &pcx,
            None,
            &tool_configs,
            &Default::default(),
        )
        .expect_err("config should fail: upward inheritance not allowed");

        // Error should be attributed to tool2 since that's where the invalid
        // inheritance is defined.
        assert_eq!(error.tool(), Some(&tool_name("tool2")));

        match error.kind() {
            ConfigParseErrorKind::InheritanceErrors(errors) => {
                assert_eq!(errors.len(), 1);
                assert!(
                    matches!(
                        &errors[0],
                        InheritsError::UnknownInheritance(from, to)
                        if from == "prof_b" && to == "prof_a"
                    ),
                    "expected UnknownInheritance(prof_b, prof_a), got {:?}",
                    errors[0]
                );
            }
            other => {
                panic!("expected ConfigParseErrorKind::InheritanceErrors, got {other}")
            }
        }
    }
}
