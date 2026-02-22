// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Specifies how test output is represented.
//!
//! The [`OutputSpec`] trait abstracts over two modes of output storage:
//!
//! - [`LiveSpec`]: output stored in memory during live execution, using
//!   [`ChildOutputDescription`].
//! - [`RecordingSpec`]: output stored in recordings,
//!   using [`ZipStoreOutputDescription`].
//!
//! Types generic over `S: OutputSpec` use `S::ChildOutputDesc` for their output
//! description fields. This enables adding additional associated types in the
//! future without changing every generic type's parameter list.

use crate::{record::ZipStoreOutputDescription, reporter::events::ChildOutputDescription};
use serde::{Serialize, de::DeserializeOwned};

/// Specifies how test output is represented.
///
/// Two implementations exist:
///
/// - [`LiveSpec`]: output stored in memory during live execution.
/// - [`RecordingSpec`]: output stored in recordings.
pub trait OutputSpec {
    /// The type used to describe child output.
    type ChildOutputDesc;
}

/// Output spec for live test execution.
///
/// Uses [`ChildOutputDescription`] for in-memory byte buffers with lazy UTF-8
/// string conversion.
pub struct LiveSpec;

impl OutputSpec for LiveSpec {
    type ChildOutputDesc = ChildOutputDescription;
}

/// Output spec for recorded/replayed test runs.
///
/// Uses [`ZipStoreOutputDescription`] for content-addressed file references in
/// zip archives.
pub struct RecordingSpec;

impl OutputSpec for RecordingSpec {
    type ChildOutputDesc = ZipStoreOutputDescription;
}

/// An [`OutputSpec`] that supports serialization and deserialization.
pub trait SerializableOutputSpec:
    OutputSpec<ChildOutputDesc: Serialize + DeserializeOwned>
{
}

impl<S> SerializableOutputSpec for S
where
    S: OutputSpec,
    S::ChildOutputDesc: Serialize + DeserializeOwned,
{
}

/// An [`OutputSpec`] that supports generation via
/// [`proptest::arbitrary::Arbitrary`].
#[cfg(test)]
pub(crate) trait ArbitraryOutputSpec:
    OutputSpec<ChildOutputDesc: proptest::arbitrary::Arbitrary + PartialEq + 'static> + 'static
{
}

#[cfg(test)]
impl<S> ArbitraryOutputSpec for S
where
    S: OutputSpec + 'static,
    S::ChildOutputDesc: proptest::arbitrary::Arbitrary + PartialEq + 'static,
{
}
