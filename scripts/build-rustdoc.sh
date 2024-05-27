#!/usr/bin/env bash

set -e -o pipefail

# Build docs for all crates and direct dependencies. The gawk script turns e.g. "quick-junit v0.1.0"
# into "quick-junit@0.1.0".
cargo tree --depth 1 -e normal --prefix none \
    | gawk '{ gsub(" v", "@", $0); printf("%s\n", $1); }' \
    | xargs printf -- '-p %s\n' \
    | xargs cargo doc --no-deps --lib
