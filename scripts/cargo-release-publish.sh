#!/bin/bash

# Use cargo-release to publish crates to crates.io.

set -xe -o pipefail

# cargo-release requires a release off a branch (maybe it shouldn't?)
# Check out a branch.
git checkout -b to-release

cargo release publish --publish --execute --no-confirm --workspace "$@"

git checkout -
git branch -D to-release
