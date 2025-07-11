#!/usr/bin/env bash

set -e -o pipefail

# Build the site with mkdocs
cd site

# If cairosvg can't find cairo library: https://t.ly/MfX6u
command -v brew > /dev/null && export DYLD_FALLBACK_LIBRARY_PATH="$(brew --prefix)/lib"

uv run mkdocs build
