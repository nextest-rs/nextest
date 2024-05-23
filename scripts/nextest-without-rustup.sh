#!/usr/bin/env bash

# Run tests without rustup or cargo wrappers.
#
# Some scenarios involve running tests without rustup or cargo. We can't run `cargo nextest run`
# directly because it will use the `cargo` wrapper installed by rustup. Instead, we try our best
# to not use rustup or cargo.
#
# * CARGO is set to $(rustup which cargo) to bypass the rustup wrapper.
# * Nextest is invoked as "cargo-nextest nextest run" rather than "cargo nextest run".

set -e -o pipefail

if [[ $RUSTUP_AVAILABLE -eq 1 ]]; then
    CARGO="$(rustup which cargo)"
    RUSTC="$(rustup which rustc)"
else
    # These paths should hopefully make it clear that cargo and rustc are not available -- if we try
    # to run them then they'll fail.
    CARGO="cargo-unavailable"
    RUSTC="rustc-unavailable"
fi

export CARGO RUSTC

export CARGO_NEXTEST=${CARGO_NEXTEST:-"cargo-nextest"}

echo "[nextest-without-rustup] nextest with CARGO_NEXTEST=$CARGO_NEXTEST" >&2
$CARGO_NEXTEST nextest "$@"
