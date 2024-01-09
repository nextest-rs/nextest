#!/usr/bin/env bash

set -euo pipefail

# Generates a JUnit fixture and copies it to nextest-tests-junit.xml.

cd "$(git rev-parse --show-toplevel)"
cargo run -p cargo-nextest -- \
    nextest run --manifest-path fixtures/nextest-tests/Cargo.toml \
    --profile with-junit -E 'test(=test_cwd) + test(=test_failure_assert) + test(=test_flaky_mod_4)' || true
cp fixtures/nextest-tests/target/nextest/with-junit/junit.xml fixtures/nextest-tests-junit.xml
