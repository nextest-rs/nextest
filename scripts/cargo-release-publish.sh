#!/bin/bash

# Use cargo-release to publish crates to crates.io.

set -xe -o pipefail

# cargo-release requires a release off a branch (maybe it shouldn't?)
# Check out this branch, creating it if it doesn't exist.
git checkout -B to-release

# Publish all crates except cargo-nextest first. Do this against main so `.cargo_vcs_info.json` is
# valid. (cargo-nextest is the only crate that cares about commit info.)
cargo release publish --publish --execute --no-confirm --workspace --exclude cargo-nextest --exclude internal-test

if [[ $PUBLISH_CARGO_NEXTEST == "1" ]]; then
    # Check that NEXTEST_TAG is set.
    if [[ -z $NEXTEST_TAG ]]; then
        echo "NEXTEST_TAG is not set"
        exit 1
    fi

    # Write out commit-related metadata. This matches cargo-nextest's build.rs.
    git log -1 --date=short --format="%H %h %cd" --abbrev=9 > cargo-nextest/nextest-commit-info

    # Making a commit here is important because cargo-release does not allow passing in
    # --allow-dirty. But note that `nextest-commit-info` is what's on main.
    #
    # This does unfortunately mean that Cargo's own `.cargo_vcs_info.json` will be incorrect, but
    # what can you do.
    git add cargo-nextest/nextest-commit-info
    # Set the Git user info so the commit doesn't fail.
    git config user.email "bot@nexte.st"
    git config user.name "Nextest Bot"
    git commit -m "Write out commit info for cargo-nextest"

    # Publish cargo-nextest.
    cargo release publish --publish --execute --no-confirm -p cargo-nextest

    # Mark the release as published (must be done as the very last step, since
    # releases are immutable).
    gh release edit $NEXTEST_TAG --draft=false
fi

git checkout -
git branch -D to-release
