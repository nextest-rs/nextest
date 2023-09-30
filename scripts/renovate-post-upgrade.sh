#!/bin/bash

set -euo pipefail

# Fetch cargo binstall.
curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash

# Install cargo-hakari.
cargo binstall cargo-hakari@0.9.27 --no-confirm

# Run cargo hakari.
cargo hakari generate
