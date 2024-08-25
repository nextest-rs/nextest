#!/usr/bin/env bash

set -e -o pipefail

# Build the site with mkdocs
cd site
uv run mkdocs build
