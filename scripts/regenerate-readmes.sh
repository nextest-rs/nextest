#!/usr/bin/env bash

# Copyright (c) The cargo-guppy Contributors
# SPDX-License-Identifier: MIT OR Apache-2.0

# Regenerate readme files in this repository.

set -eo pipefail

cd "$(git rev-parse --show-toplevel)"
git ls-files | grep README.tpl$ | while read -r readme; do
  dir=$(dirname "$readme")
  cargo readme --project-root "$dir" > "$dir/README.md"
done
