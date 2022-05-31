#!/bin/bash

# Regenerate the nextest help text displayed on the website.

set -e -o pipefail

cd "$(git rev-parse --show-toplevel)"
mkdir -p site/help-text
cargo nextest list -h > site/help-text/list-help.txt
cargo nextest run -h > site/help-text/run-help.txt
cargo nextest archive -h > site/help-text/archive-help.txt
