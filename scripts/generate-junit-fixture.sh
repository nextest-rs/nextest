#!/usr/bin/env bash

set -euo pipefail

# Generates a JUnit fixture and copies it to fixture-project-junit.xml.

cd "$(git rev-parse --show-toplevel)"
cargo run -p cargo-nextest -- \
    nextest run --manifest-path fixtures/fixture-project/Cargo.toml \
    --profile with-junit -E 'test(=test_cwd) + test(=test_failure_assert) + test(=test_flaky_mod_4)' || true
cp fixtures/fixture-project/target/nextest/with-junit/junit.xml fixtures/fixture-project-junit.xml
