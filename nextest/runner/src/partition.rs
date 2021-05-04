// Copyright (c) The diem-devtools Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Support for partitioning test runs across several machines.
//!
//! At the moment this only supports a simple hash-based sharding. In the future it could potentially
//! be made smarter: e.g. using data to pick different sets of binaries and tests to run, with
//! an aim to minimize total build and test times.

use crate::test_list::TestBinary;
use anyhow::{anyhow, bail, Context};
use std::{
    fmt,
    hash::{Hash, Hasher},
    str::FromStr,
};
use twox_hash::XxHash64;

/// A builder for creating `Partitioner` instances.
///
/// The relationship between `PartitionerBuilder` and `Partitioner` is similar to that between
/// `std`'s `BuildHasher` and `Hasher`.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PartitionerBuilder {
    /// Partition based on counting test numbers.
    Count {
        /// The shard this is in, counting up from 1.
        shard: u64,

        /// The total number of shards.
        total_shards: u64,
    },

    /// Partition based on hashing. Individual partitions are stateless.
    Hash {
        /// The shard this is in, counting up from 1.
        shard: u64,

        /// The total number of shards.
        total_shards: u64,
    },
}

/// Represents an individual partitioner, typically scoped to a test binary.
pub trait Partitioner: fmt::Debug {
    /// Returns true if the given test name matches the partition.
    fn test_matches(&mut self, test_name: &str) -> bool;
}

impl PartitionerBuilder {
    /// Creates a new `Partitioner` from this `PartitionerBuilder`.
    pub fn build(&self, _test_binary: &TestBinary) -> Box<dyn Partitioner> {
        // Note we don't use test_binary at the moment but might in the future.
        match self {
            PartitionerBuilder::Count {
                shard,
                total_shards,
            } => Box::new(CountPartitioner::new(*shard, *total_shards)),
            PartitionerBuilder::Hash {
                shard,
                total_shards,
            } => Box::new(HashPartitioner::new(*shard, *total_shards)),
        }
    }

    // ---
    // Helper methods
    // ---

    fn parse_impl(s: &str) -> anyhow::Result<Self> {
        // Parse the string: it looks like "hash:<shard>/<total_shards>".
        if let Some(input) = s.strip_prefix("hash:") {
            let (shard, total_shards) =
                parse_shards(input).context("partition must be in the format \"hash:M/N\"")?;

            Ok(PartitionerBuilder::Hash {
                shard,
                total_shards,
            })
        } else if let Some(input) = s.strip_prefix("count:") {
            let (shard, total_shards) =
                parse_shards(input).context("partition must be in the format \"count:M/N\"")?;

            Ok(PartitionerBuilder::Count {
                shard,
                total_shards,
            })
        } else {
            bail!(
                "partition input '{}' must begin with \"hash:\" or \"count:\"",
                s
            )
        }
    }
}

/// An error that occurs while parsing a `PartitionerBuilder` input.
#[derive(Debug)]
pub struct ParseError(anyhow::Error);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let chain = self.0.chain();
        let len = chain.len();
        for (i, err) in chain.enumerate() {
            if i == 0 {
                writeln!(f, "{}", err)?;
            } else if i < len - 1 {
                writeln!(f, "({})", err)?;
            } else {
                // Skip the last newline since that's what looks best with structopt.
                write!(f, "({})", err)?;
            }
        }
        Ok(())
    }
}

impl FromStr for PartitionerBuilder {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_impl(s).map_err(ParseError)
    }
}

fn parse_shards(input: &str) -> anyhow::Result<(u64, u64)> {
    let mut split = input.splitn(2, '/');
    // First "next" always returns a value.
    let shard_str = split.next().expect("split should have at least 1 element");
    // Second "next" may or may not return a value.
    let total_shards_str = split
        .next()
        .ok_or_else(|| anyhow!("expected input '{}' to be in the format M/N", input))?;

    let shard: u64 = shard_str
        .parse()
        .with_context(|| format!("failed to parse shard '{}' as u64", shard_str))?;

    let total_shards: u64 = total_shards_str
        .parse()
        .with_context(|| format!("failed to parse total_shards '{}' as u64", total_shards_str))?;

    // Check that shard > 0 and <= total_shards.
    if !(1..=total_shards).contains(&shard) {
        bail!(
            "shard {} must be a number between 1 and total shards {}, inclusive",
            shard,
            total_shards
        );
    }

    Ok((shard, total_shards))
}

#[derive(Clone, Debug)]
struct CountPartitioner {
    shard_minus_one: u64,
    total_shards: u64,
    curr: u64,
}

impl CountPartitioner {
    fn new(shard: u64, total_shards: u64) -> Self {
        let shard_minus_one = shard - 1;
        Self {
            shard_minus_one,
            total_shards,
            curr: 0,
        }
    }
}

impl Partitioner for CountPartitioner {
    fn test_matches(&mut self, _test_name: &str) -> bool {
        let matches = self.curr == self.shard_minus_one;
        self.curr = (self.curr + 1) % self.total_shards;
        matches
    }
}

#[derive(Clone, Debug)]
struct HashPartitioner {
    shard_minus_one: u64,
    total_shards: u64,
}

impl HashPartitioner {
    fn new(shard: u64, total_shards: u64) -> Self {
        let shard_minus_one = shard - 1;
        Self {
            shard_minus_one,
            total_shards,
        }
    }
}

impl Partitioner for HashPartitioner {
    fn test_matches(&mut self, test_name: &str) -> bool {
        let mut hasher = XxHash64::default();
        test_name.hash(&mut hasher);
        hasher.finish() % self.total_shards == self.shard_minus_one
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partitioner_builder_from_str() {
        let successes = vec![
            (
                "hash:1/2",
                PartitionerBuilder::Hash {
                    shard: 1,
                    total_shards: 2,
                },
            ),
            (
                "hash:1/1",
                PartitionerBuilder::Hash {
                    shard: 1,
                    total_shards: 1,
                },
            ),
            (
                "hash:99/200",
                PartitionerBuilder::Hash {
                    shard: 99,
                    total_shards: 200,
                },
            ),
        ];

        let failures = vec![
            "foo",
            "hash",
            "hash:",
            "hash:1",
            "hash:1/",
            "hash:0/2",
            "hash:3/2",
            "hash:m/2",
            "hash:1/n",
            "hash:1/2/3",
        ];

        for (input, output) in successes {
            assert_eq!(
                PartitionerBuilder::from_str(input).unwrap_or_else(|err| panic!(
                    "expected input '{}' to succeed, failed with: {}",
                    input, err
                )),
                output,
                "success case '{}' matches",
                input,
            );
        }

        for input in failures {
            PartitionerBuilder::from_str(input)
                .expect_err(&format!("expected input '{}' to fail", input));
        }
    }
}
