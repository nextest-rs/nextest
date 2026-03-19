#!/usr/bin/env bash

# Archive signed Windows binaries and upload them to the GitHub release.
#
# Required environment variables:
#   REF_NAME - the git ref name (tag).
#   TARGET   - the build target (e.g. x86_64-pc-windows-msvc).
#   GH_TOKEN - GitHub token for `gh release upload`.
#
# Usage: archive-and-upload-windows.sh <binary-name>

set -xe -o pipefail

if [[ -z "${REF_NAME}" ]]; then
    echo "REF_NAME must be set" >&2
    exit 1
fi
if [[ -z "${TARGET}" ]]; then
    echo "TARGET must be set" >&2
    exit 1
fi
if [[ -z "${1}" ]]; then
    echo "Usage: archive-and-upload-windows.sh <binary-name>" >&2
    exit 1
fi

binary_name="${1}"
prefix="${REF_NAME}-${TARGET}"

cd target/signed

tar -czf "${prefix}.tar.gz" "${binary_name}"
# Windows has 7z, not zip.
7z a "${prefix}.zip" "${binary_name}"

sha256sum --binary \
    "${prefix}.tar.gz" \
    "${prefix}.zip" \
    > "${prefix}.sha256"
b2sum --binary \
    "${prefix}.tar.gz" \
    "${prefix}.zip" \
    > "${prefix}.b2"

gh release upload "${REF_NAME}" \
    "${prefix}.tar.gz" \
    "${prefix}.zip" \
    "${prefix}.sha256" \
    "${prefix}.b2"
