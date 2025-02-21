// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::TargetTripleSource;
use crate::errors::TargetTripleError;
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;

/// Represents a custom platform that was extracted from stored metadata.
///
/// The platform is stored in a temporary directory, and is deleted when this struct is dropped.
#[derive(Debug)]
pub struct ExtractedCustomPlatform {
    source: TargetTripleSource,
    dir: Utf8TempDir,
    path: Utf8PathBuf,
}

impl ExtractedCustomPlatform {
    /// Writes the custom JSON to a temporary directory.
    pub fn new(
        triple_str: &str,
        json: &str,
        source: TargetTripleSource,
    ) -> Result<Self, TargetTripleError> {
        // Extract the JSON to a temporary file. Cargo requires that the file name be of the form
        // `<triple_str>.json`.
        let temp_dir = camino_tempfile::Builder::new()
            .prefix("nextest-custom-target-")
            .rand_bytes(5)
            .tempdir()
            .map_err(|error| TargetTripleError::CustomPlatformTempDirError {
                source: source.clone(),
                error,
            })?;

        let path = temp_dir.path().join(format!("{triple_str}.json"));

        std::fs::write(&path, json).map_err(|error| {
            TargetTripleError::CustomPlatformWriteError {
                source: source.clone(),
                path: path.clone(),
                error,
            }
        })?;

        Ok(Self {
            source,
            dir: temp_dir,
            path,
        })
    }

    /// Returns the source of the custom platform.
    pub fn source(&self) -> &TargetTripleSource {
        &self.source
    }

    /// Returns the temporary directory.
    pub fn dir(&self) -> &Utf8TempDir {
        &self.dir
    }

    /// Returns the path to the JSON file containing the custom platform.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Close the temporary directory.
    ///
    /// The directory is deleted when this struct is dropped, but this method can be used to detect
    /// errors during cleanup.
    pub fn close(self) -> Result<(), TargetTripleError> {
        let dir_path = self.dir.path().to_owned();
        self.dir
            .close()
            .map_err(|error| TargetTripleError::CustomPlatformCloseError {
                source: self.source,
                dir_path,
                error,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_config::{
        CargoTargetArg, TargetDefinitionLocation, TargetTriple, test_helpers::setup_temp_dir,
    };
    use color_eyre::eyre::{Context, Result, bail, eyre};

    #[test]
    fn test_extracted_custom_platform() -> Result<()> {
        // Integration testing a full custom platform is hard because it requires a build of std. So
        // we just do a limited unit test: use the existing custom platform fixture, and run through
        // the serialize/extract process, ensuring that the initial and final platform instances
        // produced are the same.

        let target = {
            // Put this in here to ensure that this temp dir is dropped -- once the target is read
            // there should be no further access to the temp dir.
            let temp_dir = setup_temp_dir()?;
            let platform_path = temp_dir.path().join("custom-target/my-target.json");

            // Read in the custom platform and turn it into a `TargetTriple`.
            TargetTriple::custom_from_path(
                &platform_path,
                TargetTripleSource::CliOption,
                temp_dir.path(),
            )?
        };

        // Serialize the `TargetTriple` to a `PlatformSummary`.
        let summary = target.platform.to_summary();

        // Now deserialize the `PlatformSummary` back into a `TargetTriple`.
        let target2 = TargetTriple::deserialize(Some(summary))
            .wrap_err("deserializing target triple")?
            .ok_or_else(|| eyre!("deserializing target triple resulted in None"))?;

        assert_eq!(target2.source, TargetTripleSource::Metadata);
        assert!(
            matches!(
                target2.location,
                TargetDefinitionLocation::MetadataCustom(_)
            ),
            "triple2.location should be MetadataCustom: {:?}",
            target2.location
        );

        // Now attempt to extract the custom platform.
        let arg = target2
            .to_cargo_target_arg()
            .wrap_err("converting to cargo target arg")?;
        let extracted = match &arg {
            CargoTargetArg::Extracted(extracted) => extracted,
            _ => bail!("expected CargoTargetArg::Extracted, found {:?}", arg),
        };

        // Generally ensure that Cargo will work with the extracted path.
        assert!(extracted.path().is_absolute(), "path should be absolute");
        assert!(
            extracted.path().ends_with("my-target.json"),
            "extracted path should end with 'my-target.json'"
        );
        assert_eq!(
            arg.to_string(),
            extracted.path(),
            "arg matches extracted path"
        );

        // Now, read in the path and turn it into another TargetTriple.
        let target3 = TargetTriple::custom_from_path(
            extracted.path(),
            TargetTripleSource::CliOption,
            extracted.dir().path(),
        )?;
        assert_eq!(target3.platform, target.platform, "platform roundtrips");

        Ok(())
    }
}
