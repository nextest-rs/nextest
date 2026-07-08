// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A backend paired with the policy governing how a run uses it.

use crate::cache::{CacheBackend, CachePolicy};
use std::sync::Arc;

/// The result cache as seen by a single run: the backend (if the cache is
/// enabled and available) together with the [`CachePolicy`] deciding whether
/// this run consults it, records into it, or neither.
///
/// A `None` backend means the cache is disabled (feature off, `--no-cache`, or
/// no usable cache directory). A present backend with a restrictive policy means
/// the cache exists — and is still pruned — but this particular run does not read
/// or write it.
#[derive(Clone)]
pub struct CacheHandle {
    cache: Option<Arc<dyn CacheBackend>>,
    policy: CachePolicy,
}

impl CacheHandle {
    /// Creates a handle over the given backend and policy.
    pub fn new(cache: Option<Arc<dyn CacheBackend>>, policy: CachePolicy) -> Self {
        Self { cache, policy }
    }

    /// Creates a handle for a run with no cache at all.
    pub fn disabled() -> Self {
        Self {
            cache: None,
            policy: CachePolicy::default(),
        }
    }

    /// Returns the backend to consult during listing, or `None` if this run does
    /// not read the cache.
    pub fn consult(&self) -> Option<Arc<dyn CacheBackend>> {
        self.policy.consult.then(|| self.cache.clone()).flatten()
    }

    /// Returns the backend, if the cache exists, regardless of policy.
    ///
    /// The writer takes the backend whenever it exists and defers the per-test
    /// record decision to the [`CachePolicy`]: even a run that does not record
    /// still writes touches for the entries it consulted.
    pub fn backend(&self) -> Option<Arc<dyn CacheBackend>> {
        self.cache.clone()
    }

    /// Returns the caching policy for this run.
    pub fn policy(&self) -> CachePolicy {
        self.policy
    }
}
