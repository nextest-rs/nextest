#!/bin/bash

# Run test_cargo_env_vars in the cargo test environment. This is intended for development, to get a
# list of environment variables set in the `cargo test` environment.

set -euo pipefail

export __NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_NO_OVERRIDE=test-PASSED-value-set-by-environment
export __NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_OVERRIDDEN=test-FAILED-value-set-by-environment
export __NEXTEST_ENV_VAR_FOR_TESTING_IN_PARENT_ENV_RELATIVE_OVERRIDDEN=test-FAILED-value-set-by-environment

export __NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_EXTRA=test-FAILED-value-set-by-environment
export __NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_MAIN=test-FAILED-value-set-by-environment
export __NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_IN_BOTH=test-FAILED-value-set-by-environment
export __NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_NONE=test-PASSED-value-set-by-environment
export __NEXTEST_TESTING_EXTRA_CONFIG_OVERRIDE_FORCE_FALSE=test-PASSED-value-set-by-environment

cd "$(git rev-parse --show-toplevel)"/fixtures/nextest-tests
cargo test --config .cargo/extra-config.toml -- test_cargo_env_vars
