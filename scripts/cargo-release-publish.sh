#!/usr/bin/env bash

# Use cargo-release to publish crates to crates.io.

set -xe -o pipefail

# Publish a crate with retries.
#
# Publishing a crate can fail for at least two reasons:
#
# * concurrent release jobs can race and result in 400 "already uploaded".
# * crates.io can return transient 5xx errors.
#
# This retry should hopefully catch and converge with both reasons.
publish_with_retries() {
    local attempt
    for attempt in 1 2 3; do
        if cargo release publish --publish --execute --no-confirm "$@"; then
            return 0
        fi
        if [[ "${attempt}" -lt 3 ]]; then
            sleep $((attempt * 15))
        fi
    done
    echo "error: failed to publish after 3 attempts" >&2
    return 1
}

# cargo-release requires a release off a branch (maybe it shouldn't?)
# Check out this branch, creating it if it doesn't exist.
git checkout -B to-release

# Publish all crates except cargo-nextest first. Do this against main so `.cargo_vcs_info.json` is
# valid. (cargo-nextest is the only crate that cares about commit info.)
publish_with_retries --workspace --exclude cargo-nextest --exclude internal-test

if [[ $PUBLISH_CARGO_NEXTEST == "1" ]]; then
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
    publish_with_retries -p cargo-nextest
fi

git checkout -
git branch -D to-release
