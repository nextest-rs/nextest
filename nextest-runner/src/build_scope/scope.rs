// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::PackageSpec;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A build scope for Cargo.
///
/// For more information, see [_Build scope
/// arguments_](https://nexte.st/docs/design/architecture/recording-runs/#build-scope-arguments)
/// on the nextest site.
// NOTE: Unknown fields are ignored on read. If you add a new field, ensure that
// RUNS_JSON_FORMAT_VERSION and STORE_FORMAT_VERSION (minor version) are both
// bumped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BuildScope {
    #[serde(
        default = "PackageSelection::empty",
        skip_serializing_if = "PackageSelection::is_empty"
    )]
    packages: PackageSelection,
    #[serde(
        default = "TargetSelection::empty",
        skip_serializing_if = "TargetSelection::is_empty"
    )]
    targets: TargetSelection,
    #[serde(
        default = "FeatureSelection::empty",
        skip_serializing_if = "FeatureSelection::is_empty"
    )]
    features: FeatureSelection,
}

impl BuildScope {
    /// Returns a new [`BuildScope`] with the given package, target, and feature
    /// selections.
    pub fn new(
        packages: PackageSelection,
        targets: TargetSelection,
        features: FeatureSelection,
    ) -> Self {
        Self {
            packages,
            targets,
            features,
        }
    }

    /// Returns the package selection for this scope.
    pub fn packages(&self) -> &PackageSelection {
        &self.packages
    }

    /// Returns the target selection for this scope.
    pub fn targets(&self) -> &TargetSelection {
        &self.targets
    }

    /// Returns the feature selection for this scope.
    pub fn features(&self) -> &FeatureSelection {
        &self.features
    }

    /// Returns `true` if this scope is empty, i.e. no package, target, or
    /// feature selections.
    ///
    /// This results in Cargo's default scope for the current directory.
    pub fn is_empty(&self) -> bool {
        let Self {
            packages,
            targets,
            features,
        } = self;
        packages.is_empty() && targets.is_empty() && features.is_empty()
    }

    /// Returns flags to provide to Cargo.
    pub fn cargo_flags(&self) -> Vec<CargoFlag<'_>> {
        let Self {
            packages,
            targets,
            features,
        } = self;

        let mut flags = Vec::new();
        packages.push_cargo_flags(&mut flags);
        targets.push_cargo_flags(&mut flags);
        features.push_cargo_flags(&mut flags);
        flags
    }

    /// Returns the set of arguments to pass to Cargo.
    ///
    /// Flags with values are always returned as a single `--flag=value`
    /// argument, so that values starting with `-` aren't interpreted as flags.
    pub fn to_cargo_args(&self) -> Vec<String> {
        self.cargo_flags()
            .into_iter()
            .map(|flag| flag.to_cargo_arg())
            .collect()
    }
}

impl fmt::Display for BuildScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, flag) in self.cargo_flags().into_iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            match flag {
                CargoFlag::Bare(flag) => f.write_str(flag)?,
                CargoFlag::Valued { flag, value } => {
                    // Quote the value for friendlier user display. Flag names
                    // never need to be quoted since they don't have embedded
                    // spaces or special characters.
                    write!(f, "{flag}={}", shell_words::quote(value))?
                }
            }
        }
        Ok(())
    }
}

/// A single Cargo flag, either with or without a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CargoFlag<'a> {
    /// A bare flag without a value, e.g. `--workspace`.
    Bare(&'static str),
    /// A valued flag with a value, e.g. `--package=foo`.
    Valued {
        /// The flag name, e.g. `--package`.
        flag: &'static str,
        /// The flag value, e.g. `foo`.
        value: &'a str,
    },
}

impl CargoFlag<'_> {
    /// Converts this flag to a Cargo argument string, e.g. `--package=foo`.
    pub fn to_cargo_arg(&self) -> String {
        match self {
            Self::Bare(flag) => (*flag).to_owned(),
            // We shouldn't quote the value here, because this is passed into the
            // subprocess's argv directly without going through a shell.
            Self::Valued { flag, value } => format!("{flag}={value}"),
        }
    }
}

/// Package selection in a build scope.
///
/// The deprecated `--all` is not represented here; it is equivalent to
/// `workspace`, and is folded while converting from the CLI arguments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PackageSelection {
    /// The package specs to include in the build.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<PackageSpec>,
    /// Whether to include the entire workspace in the build.
    #[serde(default, skip_serializing_if = "is_false")]
    pub workspace: bool,
    /// The package specs to exclude from the build.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<PackageSpec>,
}

impl PackageSelection {
    fn empty() -> Self {
        Self {
            specs: Vec::new(),
            workspace: false,
            exclude: Vec::new(),
        }
    }

    /// Returns `true` if this package selection is empty.
    pub fn is_empty(&self) -> bool {
        let Self {
            specs,
            workspace,
            exclude,
        } = self;
        specs.is_empty() && !*workspace && exclude.is_empty()
    }

    fn push_cargo_flags<'a>(&'a self, flags: &mut Vec<CargoFlag<'a>>) {
        let Self {
            specs,
            workspace,
            exclude,
        } = self;

        for spec in specs {
            flags.push(CargoFlag::Valued {
                flag: "--package",
                value: spec.as_str(),
            });
        }
        if *workspace {
            flags.push(CargoFlag::Bare("--workspace"));
        }
        for spec in exclude {
            flags.push(CargoFlag::Valued {
                flag: "--exclude",
                value: spec.as_str(),
            });
        }
    }
}

/// Target selection in a build scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TargetSelection {
    /// Whether to include the library target in the build.
    #[serde(default, skip_serializing_if = "is_false")]
    pub lib: bool,

    /// The binary targets to include in the build.
    #[serde(
        default = "NamedTargets::empty",
        skip_serializing_if = "NamedTargets::is_empty"
    )]
    pub bins: NamedTargets,

    /// The example targets to include in the build.
    #[serde(
        default = "NamedTargets::empty",
        skip_serializing_if = "NamedTargets::is_empty"
    )]
    pub examples: NamedTargets,

    /// The test targets to include in the build.
    #[serde(
        default = "NamedTargets::empty",
        skip_serializing_if = "NamedTargets::is_empty"
    )]
    pub tests: NamedTargets,

    /// The benchmark targets to include in the build.
    #[serde(
        default = "NamedTargets::empty",
        skip_serializing_if = "NamedTargets::is_empty"
    )]
    pub benches: NamedTargets,

    /// Whether to include all targets in the build.
    #[serde(default, skip_serializing_if = "is_false")]
    pub all_targets: bool,
}

impl TargetSelection {
    fn empty() -> Self {
        Self {
            lib: false,
            bins: NamedTargets::empty(),
            examples: NamedTargets::empty(),
            tests: NamedTargets::empty(),
            benches: NamedTargets::empty(),
            all_targets: false,
        }
    }

    /// Returns `true` if this target selection is empty.
    pub fn is_empty(&self) -> bool {
        let Self {
            lib,
            bins,
            examples,
            tests,
            benches,
            all_targets,
        } = self;
        !*lib
            && bins.is_empty()
            && examples.is_empty()
            && tests.is_empty()
            && benches.is_empty()
            && !*all_targets
    }

    fn push_cargo_flags<'a>(&'a self, flags: &mut Vec<CargoFlag<'a>>) {
        let Self {
            lib,
            bins,
            examples,
            tests,
            benches,
            all_targets,
        } = self;

        if *lib {
            flags.push(CargoFlag::Bare("--lib"));
        }
        bins.push_cargo_flags(flags, "--bin", "--bins");
        examples.push_cargo_flags(flags, "--example", "--examples");
        tests.push_cargo_flags(flags, "--test", "--tests");
        benches.push_cargo_flags(flags, "--bench", "--benches");
        if *all_targets {
            flags.push(CargoFlag::Bare("--all-targets"));
        }
    }
}

/// A named target selection in a build scope, for `--bin`/`--bins`,
/// `--example`/`--examples`, `--test`/`--tests`, and `--bench`/`--benches`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct NamedTargets {
    /// The names of the targets to include, e.g. `--bin=NAME`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,

    /// Whether to include all targets of this kind, e.g. `--bins`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub all: bool,
}

impl NamedTargets {
    pub(crate) fn empty() -> Self {
        Self {
            names: Vec::new(),
            all: false,
        }
    }

    /// Returns `true` if this target selection is empty.
    pub fn is_empty(&self) -> bool {
        let Self { names, all } = self;
        names.is_empty() && !*all
    }

    fn push_cargo_flags<'a>(
        &'a self,
        flags: &mut Vec<CargoFlag<'a>>,
        name_flag: &'static str,
        all_flag: &'static str,
    ) {
        let Self { names, all } = self;

        for name in names {
            flags.push(CargoFlag::Valued {
                flag: name_flag,
                value: name,
            });
        }
        if *all {
            flags.push(CargoFlag::Bare(all_flag));
        }
    }
}

/// A selection of features to include in a build scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FeatureSelection {
    /// The features to include, e.g. `--feature=NAME`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Whether to include all features: `--all-features`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub all_features: bool,
    /// Whether to exclude default features: `--no-default-features`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_default_features: bool,
}

impl FeatureSelection {
    fn empty() -> Self {
        Self {
            features: Vec::new(),
            all_features: false,
            no_default_features: false,
        }
    }

    /// Returns `true` if this feature selection is empty.
    pub fn is_empty(&self) -> bool {
        let Self {
            features,
            all_features,
            no_default_features,
        } = self;
        features.is_empty() && !*all_features && !*no_default_features
    }

    fn push_cargo_flags<'a>(&'a self, flags: &mut Vec<CargoFlag<'a>>) {
        let Self {
            features,
            all_features,
            no_default_features,
        } = self;

        for feature in features {
            flags.push(CargoFlag::Valued {
                flag: "--features",
                value: feature,
            });
        }
        if *all_features {
            flags.push(CargoFlag::Bare("--all-features"));
        }
        if *no_default_features {
            flags.push(CargoFlag::Bare("--no-default-features"));
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_scope::test_helpers::arb_valid_package_spec;
    use proptest::{collection::vec, prelude::*};
    use test_strategy::proptest;

    fn spec(input: &str) -> PackageSpec {
        PackageSpec::new(input).expect("spec is valid")
    }

    fn named(names: &[&str], all: bool) -> NamedTargets {
        NamedTargets {
            names: names.iter().map(|name| (*name).to_owned()).collect(),
            all,
        }
    }

    fn populated_scope() -> BuildScope {
        BuildScope::new(
            PackageSelection {
                specs: vec![spec("foo"), spec("foo@1.2"), spec("nextest-*")],
                workspace: true,
                exclude: vec![spec("bar"), spec("baz@0.1.0")],
            },
            TargetSelection {
                lib: true,
                bins: named(&["my-bin", "bin with space", "back\\slash"], true),
                examples: named(&["example", ""], true),
                tests: named(&["integration", "-dash-test", "line\nbreak"], true),
                benches: named(&["bench"], true),
                all_targets: true,
            },
            FeatureSelection {
                features: vec![
                    "feat-a".to_owned(),
                    "feat-b feat-c".to_owned(),
                    "-feat-d,feat-e".to_owned(),
                    "it's".to_owned(),
                ],
                all_features: true,
                no_default_features: true,
            },
        )
    }

    fn empty_scope() -> BuildScope {
        BuildScope::new(
            PackageSelection::empty(),
            TargetSelection::empty(),
            FeatureSelection::empty(),
        )
    }

    #[test]
    fn serialize_populated() {
        let scope = populated_scope();
        assert!(!scope.is_empty());
        let json = serde_json::to_string_pretty(&scope).expect("serialized scope");
        insta::assert_snapshot!("serialize_populated", json);

        let json = serde_json::to_string_pretty(&empty_scope()).expect("serialized empty scope");
        insta::assert_snapshot!(json, @"{}");

        // Unknown fields are ignored on read.
        let deserialized: BuildScope = serde_json::from_str(
            r#"{"packages": {"specs": ["foo"], "future": 1}, "future": true}"#,
        )
        .expect("scope with unknown fields deserializes");
        assert_eq!(
            deserialized,
            BuildScope::new(
                PackageSelection {
                    specs: vec![spec("foo")],
                    workspace: false,
                    exclude: Vec::new(),
                },
                TargetSelection::empty(),
                FeatureSelection::empty(),
            )
        );
    }

    #[test]
    fn cargo_args_and_display() {
        let scope = populated_scope();
        let args =
            serde_json::to_string_pretty(&scope.to_cargo_args()).expect("serialized cargo args");
        insta::assert_snapshot!("cargo_args_populated", args);
        insta::assert_snapshot!("display_populated", scope.to_string());
    }

    fn arb_named_targets() -> impl Strategy<Value = NamedTargets> {
        let non_empty = (vec(any::<String>(), 0..3), any::<bool>())
            .prop_map(|(names, all)| NamedTargets { names, all });
        prop_oneof![1 => Just(NamedTargets::empty()), 2 => non_empty]
    }

    fn arb_build_scope() -> impl Strategy<Value = BuildScope> {
        let packages = (
            vec(arb_valid_package_spec(), 0..3),
            any::<bool>(),
            vec(arb_valid_package_spec(), 0..3),
        )
            .prop_map(|(specs, workspace, exclude)| PackageSelection {
                specs,
                workspace,
                exclude,
            });
        let targets = (
            any::<bool>(),
            arb_named_targets(),
            arb_named_targets(),
            arb_named_targets(),
            arb_named_targets(),
            any::<bool>(),
        )
            .prop_map(|(lib, bins, examples, tests, benches, all_targets)| {
                TargetSelection {
                    lib,
                    bins,
                    examples,
                    tests,
                    benches,
                    all_targets,
                }
            });
        let features = (vec(any::<String>(), 0..3), any::<bool>(), any::<bool>()).prop_map(
            |(features, all_features, no_default_features)| FeatureSelection {
                features,
                all_features,
                no_default_features,
            },
        );
        (
            prop_oneof![1 => Just(PackageSelection::empty()), 2 => packages],
            prop_oneof![1 => Just(TargetSelection::empty()), 2 => targets],
            prop_oneof![1 => Just(FeatureSelection::empty()), 2 => features],
        )
            .prop_map(|(packages, targets, features)| BuildScope::new(packages, targets, features))
    }

    #[proptest]
    fn serde_round_trip(#[strategy(arb_build_scope())] scope: BuildScope) {
        let json = serde_json::to_string(&scope).expect("serialized scope");
        prop_assert_eq!(scope.is_empty(), json == "{}");
        let deserialized: BuildScope = serde_json::from_str(&json).expect("deserialized scope");
        prop_assert_eq!(&deserialized, &scope);
        prop_assert_eq!(scope.is_empty(), scope.to_cargo_args().is_empty());
        let split = shell_words::split(&scope.to_string()).expect("display output splits");
        prop_assert_eq!(split, scope.to_cargo_args());
    }
}
