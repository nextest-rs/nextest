#!/usr/bin/env bash
# Copyright (c) The nextest Contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Runs this project's tests with `buck2 test`, driving nextest.
#
# Any arguments are passed through to `buck2 test`:
#
#     ./run.sh //:demo-lib-test
#     ./run.sh //... -- -E 'binary_id(root//:demo-lib-test)'
#     ./run.sh //... --test-executor-stderr=-   # also show nextest's own output
#
# Set BUCK2_NEXTEST to use an already-built executor instead of building one.

set -euo pipefail

cd "$(dirname "$0")"

executor="${BUCK2_NEXTEST:-}"
if [[ -z "$executor" ]]; then
    cargo build --quiet --package buck2-nextest
    # Buck2 needs an absolute path, and where the binary lands depends on the
    # checkout, which is why this is not in `.buckconfig`.
    target_dir="${CARGO_TARGET_DIR:-$(cd ../.. && pwd)/target}"
    executor="$target_dir/debug/buck2-nextest"
fi

if [[ ! -x "$executor" ]]; then
    echo "no buck2-nextest binary at $executor" >&2
    exit 1
fi

# Default to every target, but let an argument override it.
if [[ $# -eq 0 ]]; then
    set -- //...
fi

exec buck2 test -c "test.v2_test_executor=$executor" "$@"
