set positional-arguments

# Note: help messages should be 1 line long as required by just.

# Print a help message.
help:
    just --list

# Get the signing key slug to use on Windows, given a tag's ref_name.
win-signing-policy-slug ref_name:
    #!/usr/bin/env bash
    set -euxo pipefail

    # Extract e.g. 0.9.97-b.1 from cargo-nextest-0.9.97-b.1
    ref_name={{ref_name}}
    release_name="${ref_name#cargo-nextest-}"

    # Check if the prefix was actually removed
    if [[ "$release_name" == "$ref_name" ]]; then
        echo "Error: ref_name doesn't start with 'cargo-nextest-'" >&2
        exit 1
    fi

    # releases and -rc.N prereleases use the release-signing key, everything else
    # uses the test-signing key.
    if [[ "$release_name" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]]; then
        echo "signing-policy-slug=release-signing"
    else
        echo "signing-policy-slug=test-signing"
    fi
