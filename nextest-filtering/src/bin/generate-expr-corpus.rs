// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use nextest_filtering::ParsedExpr;
use proptest::{
    strategy::{Strategy, ValueTree},
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use std::{
    hash::{Hash, Hasher},
    path::Path,
};
use xxhash_rust::xxh3::Xxh3;

static CORPUS_DIR: &str = "fuzz/corpus/fuzz_parsing";

fn main() {
    let mut generator = ValueGenerator::from_seed("fuzz_parsing_corpus");
    for n in 0..1024 {
        let value = generator.generate(ParsedExpr::strategy());
        let path = Path::new(CORPUS_DIR);
        let path = path.join(format!("seed-{n}"));
        std::fs::write(path, value.to_string()).unwrap();
    }
}

// The below is copied from guppy. TODO is to move it out into its own crate.

/// Context for generating single values out of strategies.
///
/// Proptest is designed to be built around "value trees", which represent a spectrum from complex
/// values to simpler ones. But in some contexts, like benchmarking or generating corpuses, one just
/// wants a single value. This is a convenience struct for that.
#[derive(Debug, Default)]
pub struct ValueGenerator {
    runner: TestRunner,
}

impl ValueGenerator {
    /// Creates a new value generator with the default RNG.
    pub fn new() -> Self {
        Self {
            runner: TestRunner::new(Config::default()),
        }
    }

    /// Creates a new value generator with a deterministic RNG.
    ///
    /// This generator has a hardcoded seed, so its results are predictable across test runs.
    /// However, a new proptest version may change the seed.
    pub fn deterministic() -> Self {
        Self {
            runner: TestRunner::deterministic(),
        }
    }

    /// Creates a new value generator from the given seed.
    ///
    /// This generator is typically used with a hardcoded seed that is keyed on the input data
    /// somehow. For example, for a test fixture it may be the name of the fixture.
    pub fn from_seed(seed: impl Hash) -> Self {
        // Convert the input seed into a 32-byte hash.
        let mut rng_seed = [0u8; 32];
        for hash_seed in 0usize..=3 {
            let mut hasher = Xxh3::with_seed(hash_seed as u64);
            seed.hash(&mut hasher);
            rng_seed[hash_seed..(hash_seed + 8)].copy_from_slice(&hasher.finish().to_be_bytes());
        }

        Self {
            runner: TestRunner::new_with_rng(
                Config::default(),
                TestRng::from_seed(RngAlgorithm::default(), &rng_seed),
            ),
        }
    }

    /// Does a "partial clone" of the `ValueGenerator`, creating a new independent but deterministic
    /// RNG.
    pub fn partial_clone(&mut self) -> Self {
        Self {
            runner: TestRunner::new_with_rng(Config::default(), self.runner.new_rng()),
        }
    }

    /// Generates a single value for this strategy.
    ///
    /// Panics if generating the new value fails. The only situation in which this can happen is if
    /// generating the value causes too many internal rejects.
    pub fn generate<S: Strategy>(&mut self, strategy: S) -> S::Value {
        strategy
            .new_tree(&mut self.runner)
            .expect("creating a new value should succeed")
            .current()
    }
}
