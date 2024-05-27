#!/usr/bin/env bash

set -e -o pipefail

# Build the site with mkdocs
cd site
rye sync
source .venv/bin/activate
mkdocs build
