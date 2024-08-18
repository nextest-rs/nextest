#!/bin/bash

set -euo pipefail

# Function to retry a command up to 3 times.
function retry_command {
    local retries=3
    local delay=5
    local count=0
    until "$@"; do
        exit_code=$?
        count=$((count+1))
        if [ $count -lt $retries ]; then
            echo "Command failed with exit code $exit_code. Retrying in $delay seconds..."
            sleep $delay
        else
            echo "Command failed with exit code $exit_code after $count attempts."
            return $exit_code
        fi
    done
}

# If cargo isn't present, skip this -- it implies that a non-Rust dependency was
# updated.
if ! command -v cargo &> /dev/null; then
    echo "Skipping cargo-hakari update because cargo is not present."
    exit 0
fi

# Download and install cargo-hakari if it is not already installed.
if ! command -v cargo-hakari &> /dev/null; then
    # Need cargo-binstall to install cargo-hakari.
    if ! command -v cargo-binstall &> /dev/null; then
        # Fetch cargo binstall.
        echo "Installing cargo-binstall..."
        tempdir=$(mktemp -d)
        curl --retry 3 -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh -o "$tempdir"/install-from-binstall-release.sh
        retry_command bash "$tempdir"/install-from-binstall-release.sh
        rm -rf "$tempdir"
    fi

    # Install cargo-hakari.
    echo "Installing cargo-hakari..."
    retry_command cargo binstall cargo-hakari --no-confirm
fi

# Run cargo hakari to regenerate the workspace-hack file.
echo "Running cargo-hakari..."
cargo hakari generate
