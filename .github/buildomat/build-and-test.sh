#!/bin/env bash

# Build and test job: follows the same set of steps as the GitHub Action (ci.yml).

set -o errexit
set -o pipefail
set -o xtrace

PLATFORM="$1"

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

banner local-nt
ptime -m cargo local-nt run --profile ci

banner release
ptime -m cargo nextest run --profile ci
