#!/bin/env bash

# Build and test job: follows the same set of steps as the GitHub Action (ci.yml).

set -o errexit
set -o pipefail
set -o xtrace

PLATFORM="$1"

# Enable ANSI colors in Cargo and nextest output.
export CARGO_TERM_COLOR=always

cargo --version
rustc --version

# Install nextest
banner install
mkdir -p "${CARGO_HOME:-$HOME/.cargo}/bin"
curl -LsSf https://get.nexte.st/latest/"$PLATFORM" | gunzip | tar xf - -C "${CARGO_HOME:-$HOME/.cargo}/bin"

banner metadata
ptime -m cargo build --package nextest-metadata

banner no-update
ptime -m cargo build --package cargo-nextest --no-default-features --features default-no-update

banner nextest
ptime -m cargo build --package cargo-nextest

banner all targets
ptime -m cargo build --all-targets

banner all features
ptime -m cargo build --all-features

banner doctests
ptime -m cargo test --doc

# Create a user config file that enables test recording.
RECORDING_CONFIG_DIR="/tmp/nextest-recording-config"
RECORDING_CONFIG="$RECORDING_CONFIG_DIR/config.toml"
NEXTEST_STATE_DIR="$(mktemp -d /tmp/nextest-state.XXXXXX)"
ARCHIVE_PATH="/tmp/nextest-run-archive.zip"

mkdir -p "$RECORDING_CONFIG_DIR"
printf '[experimental]\nrecord = true\n\n[record]\nenabled = true\n' \
    > "$RECORDING_CONFIG"

export NEXTEST_STATE_DIR

banner local-nt
# Capture the exit code so we can export recordings even on test failure.
LOCAL_NT_EXIT=0
ptime -m cargo local-nt run --profile ci \
    --user-config-file "$RECORDING_CONFIG" \
    || LOCAL_NT_EXIT=$?

banner export-recording
# Export the recording archive regardless of whether the test step succeeded.
if ! ptime -m cargo local-nt store export latest \
    --user-config-file "$RECORDING_CONFIG" \
    --archive-file "$ARCHIVE_PATH"; then
    echo "warning: failed to export recording archive" >&2
fi

# Propagate the original test failure if local-nt exited nonzero.
if [[ "$LOCAL_NT_EXIT" -ne 0 ]]; then
    echo "error: cargo local-nt run failed with exit code $LOCAL_NT_EXIT" >&2
    exit "$LOCAL_NT_EXIT"
fi

banner release
ptime -m cargo nextest run --profile ci
