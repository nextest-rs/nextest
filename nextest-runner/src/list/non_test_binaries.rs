// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use guppy::PackageId;
use nextest_metadata::{RustNonTestBinaryKind, RustNonTestBinarySummary};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// A collection of non-test binaries that are part of a build.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RustNonTestBinaries {
    by_package_id: BTreeMap<PackageId, BTreeSet<RustNonTestBinarySummary>>,
}

impl RustNonTestBinaries {
    pub(crate) fn from_summary(
        summary: BTreeMap<String, BTreeSet<RustNonTestBinarySummary>>,
    ) -> Self {
        Self {
            by_package_id: summary
                .into_iter()
                .map(|(package_id, files)| (PackageId::new(package_id), files))
                .collect(),
        }
    }

    pub(crate) fn to_summary(&self) -> BTreeMap<String, BTreeSet<RustNonTestBinarySummary>> {
        self.by_package_id
            .iter()
            .map(|(package_id, files)| (package_id.repr().to_owned(), files.clone()))
            .collect()
    }

    pub(crate) fn insert(&mut self, package_id: PackageId, file: RustNonTestBinarySummary) {
        self.by_package_id
            .entry(package_id)
            .or_default()
            .insert(file);
    }

    /// Returns the number of distinct (package ID, name, kind) pairs of
    /// non-test binaries in this collection.
    ///
    /// One binary can be stored as several files sharing a name and kind (e.g.,
    /// on Windows, a dylib comes with an import library, an export library, and
    /// a .pdb). This function treats that as a single binary.
    pub(crate) fn binary_count(&self) -> usize {
        self.by_package_id.values().map(binary_count).sum()
    }

    pub(crate) fn files(&self) -> impl Iterator<Item = &RustNonTestBinarySummary> {
        self.by_package_id.values().flatten()
    }

    pub(crate) fn files_for_package(
        &self,
        package_id: &PackageId,
    ) -> impl Iterator<Item = &RustNonTestBinarySummary> {
        self.by_package_id.get(package_id).into_iter().flatten()
    }

    /// Partitions the non-test binaries in this collection for use with an archive.
    ///
    /// This drops package-scoped binaries owned by packages with no archived test binary.
    pub(crate) fn partition_for_archive(
        &self,
        relevant_package_ids: &HashSet<&str>,
    ) -> PartitionedNonTestBinaries {
        self.partition(|package_id, binary| {
            relevant_package_ids.contains(package_id.repr()) || !is_package_scoped(&binary.kind)
        })
    }

    fn partition(
        &self,
        mut retain: impl FnMut(&PackageId, &RustNonTestBinarySummary) -> bool,
    ) -> PartitionedNonTestBinaries {
        let mut by_package_id = BTreeMap::new();
        let mut filtered_out_binary_count = 0;

        for (package_id, files) in &self.by_package_id {
            let retained: BTreeSet<_> = files
                .iter()
                .filter(|file| retain(package_id, file))
                .cloned()
                .collect();

            filtered_out_binary_count += binary_count(files) - binary_count(&retained);
            // If nothing was retained, don't bother adding it to the result.
            if !retained.is_empty() {
                by_package_id.insert(package_id.clone(), retained);
            }
        }

        PartitionedNonTestBinaries {
            retained: Self { by_package_id },
            filtered_out_binary_count,
        }
    }
}

pub(crate) struct PartitionedNonTestBinaries {
    pub(crate) retained: RustNonTestBinaries,
    pub(crate) filtered_out_binary_count: usize,
}

fn binary_count(files: &BTreeSet<RustNonTestBinarySummary>) -> usize {
    files
        .iter()
        .map(|file| (file.name.as_str(), &file.kind))
        .collect::<BTreeSet<_>>()
        .len()
}

/// Returns true if this non-test binary is package-scoped.
///
/// This returns true for bin-exes (via `CARGO_BIN_EXE_<name>`) and false for
/// other kinds.
fn is_package_scoped(kind: &RustNonTestBinaryKind) -> bool {
    *kind == RustNonTestBinaryKind::BIN_EXE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_count_is_platform_stable() {
        let unix = RustNonTestBinaries::from_summary(BTreeMap::from([
            (
                "fixture-project".to_owned(),
                BTreeSet::from([
                    bin_exe("fixture-project", "debug/fixture-project"),
                    bin_exe("other", "debug/other"),
                    bin_exe("wrapper", "debug/wrapper"),
                ]),
            ),
            (
                "dylib-test".to_owned(),
                BTreeSet::from([dylib("dylib_test", "debug/libdylib_test.so")]),
            ),
        ]));

        let windows = RustNonTestBinaries::from_summary(BTreeMap::from([
            (
                "fixture-project".to_owned(),
                BTreeSet::from([
                    bin_exe("fixture-project", "debug/fixture-project.exe"),
                    bin_exe("other", "debug/other.exe"),
                    bin_exe("wrapper", "debug/wrapper.exe"),
                ]),
            ),
            (
                "dylib-test".to_owned(),
                windows_dylib("dylib_test").into_iter().collect(),
            ),
        ]));

        assert_eq!(
            unix.files().count(),
            4,
            "on Unix, each binary is stored as exactly one file"
        );
        assert_eq!(
            unix.binary_count(),
            4,
            "3 executables and 1 dylib, spread across 2 packages"
        );

        assert_eq!(
            windows.files().count(),
            7,
            "on Windows, the dylib is stored as 4 files"
        );
        assert_eq!(
            windows.binary_count(),
            4,
            "the same 4 binaries: a dylib's import library, export library, and \
             .pdb are stored as separate files but are not separate binaries"
        );
    }

    #[test]
    fn partition_counts_binaries() {
        let non_test_binaries = RustNonTestBinaries::from_summary(BTreeMap::from([
            (
                "with-tests".to_owned(),
                BTreeSet::from([bin_exe("helper", "debug/helper")]),
            ),
            (
                "three-bins".to_owned(),
                BTreeSet::from([
                    bin_exe("one", "debug/one"),
                    bin_exe("two", "debug/two"),
                    bin_exe("three", "debug/three"),
                ]),
            ),
            (
                "mixed".to_owned(),
                BTreeSet::from([
                    bin_exe("mixed-bin", "debug/mixed-bin"),
                    dylib("mixed_dylib", "debug/libmixed_dylib.so"),
                ]),
            ),
            (
                "dylib-only".to_owned(),
                BTreeSet::from([dylib("only_dylib", "debug/libonly_dylib.so")]),
            ),
            (
                "windows-dylib-only".to_owned(),
                windows_dylib("win_dylib").into_iter().collect(),
            ),
        ]));

        let partitioned = non_test_binaries
            .partition(|package_id, _| matches!(package_id.repr(), "with-tests" | "mixed"));

        assert_eq!(
            partitioned.retained.to_summary(),
            BTreeMap::from([
                (
                    "with-tests".to_owned(),
                    BTreeSet::from([bin_exe("helper", "debug/helper")]),
                ),
                (
                    "mixed".to_owned(),
                    BTreeSet::from([
                        bin_exe("mixed-bin", "debug/mixed-bin"),
                        dylib("mixed_dylib", "debug/libmixed_dylib.so"),
                    ]),
                ),
            ]),
            "retained packages keep all of their files"
        );
        assert_eq!(
            partitioned.retained.binary_count(),
            3,
            "retained binaries are counted the same way as filtered-out ones"
        );
        assert_eq!(
            partitioned.filtered_out_binary_count, 5,
            "filtered-out count is a count of binaries (3 + 1 + 1), not of packages (3) \
             or of files (3 + 1 + 4)"
        );
    }

    #[test]
    fn partition_for_archive_only_scopes_bin_exes() {
        let non_test_binaries = RustNonTestBinaries::from_summary(BTreeMap::from([
            (
                "with-tests".to_owned(),
                BTreeSet::from([bin_exe("helper", "debug/helper")]),
            ),
            (
                "three-bins".to_owned(),
                BTreeSet::from([
                    bin_exe("one", "debug/one"),
                    bin_exe("two", "debug/two"),
                    bin_exe("three", "debug/three"),
                ]),
            ),
            (
                "mixed".to_owned(),
                BTreeSet::from([
                    bin_exe("mixed-bin", "debug/mixed-bin"),
                    dylib("mixed_dylib", "debug/libmixed_dylib.so"),
                ]),
            ),
            (
                "mixed-no-tests".to_owned(),
                BTreeSet::from([
                    bin_exe("untested-bin", "debug/untested-bin"),
                    dylib("untested_dylib", "debug/libuntested_dylib.so"),
                ]),
            ),
            (
                "dylib-only".to_owned(),
                BTreeSet::from([dylib("only_dylib", "debug/libonly_dylib.so")]),
            ),
            (
                "unknown-only".to_owned(),
                BTreeSet::from([future_kind(
                    "from_a_newer_nextest",
                    "debug/from_a_newer_nextest",
                )]),
            ),
        ]));

        let relevant_package_ids = HashSet::from(["with-tests", "mixed"]);
        let partitioned = non_test_binaries.partition_for_archive(&relevant_package_ids);

        assert_eq!(
            partitioned.retained.to_summary(),
            BTreeMap::from([
                (
                    "with-tests".to_owned(),
                    BTreeSet::from([bin_exe("helper", "debug/helper")]),
                ),
                (
                    "mixed".to_owned(),
                    BTreeSet::from([
                        bin_exe("mixed-bin", "debug/mixed-bin"),
                        dylib("mixed_dylib", "debug/libmixed_dylib.so"),
                    ]),
                ),
                (
                    "mixed-no-tests".to_owned(),
                    BTreeSet::from([dylib("untested_dylib", "debug/libuntested_dylib.so")]),
                ),
                (
                    "dylib-only".to_owned(),
                    BTreeSet::from([dylib("only_dylib", "debug/libonly_dylib.so")]),
                ),
                (
                    "unknown-only".to_owned(),
                    BTreeSet::from([future_kind(
                        "from_a_newer_nextest",
                        "debug/from_a_newer_nextest",
                    )]),
                ),
            ]),
            "only bin-exes are package-scoped: dylibs and unrecognized kinds are always retained, \
             and packages left with no binaries are dropped"
        );
        assert_eq!(
            partitioned.filtered_out_binary_count, 4,
            "3 bin-exes are dropped from three-bins and 1 from mixed-no-tests; counting map \
             entries that vanished, as the old code did, would report 1"
        );
    }

    fn bin_exe(name: &str, path: &str) -> RustNonTestBinarySummary {
        RustNonTestBinarySummary {
            name: name.to_owned(),
            kind: RustNonTestBinaryKind::BIN_EXE,
            path: path.into(),
        }
    }

    fn dylib(name: &str, path: &str) -> RustNonTestBinarySummary {
        RustNonTestBinarySummary {
            name: name.to_owned(),
            kind: RustNonTestBinaryKind::DYLIB,
            path: path.into(),
        }
    }

    fn future_kind(name: &str, path: &str) -> RustNonTestBinarySummary {
        RustNonTestBinarySummary {
            name: name.to_owned(),
            kind: RustNonTestBinaryKind::new("some-future-kind"),
            path: path.into(),
        }
    }

    fn windows_dylib(name: &str) -> [RustNonTestBinarySummary; 4] {
        ["dll", "dll.lib", "dll.exp", "pdb"].map(|extension| RustNonTestBinarySummary {
            name: name.to_owned(),
            kind: RustNonTestBinaryKind::DYLIB,
            path: format!("debug/{name}.{extension}").into(),
        })
    }
}
