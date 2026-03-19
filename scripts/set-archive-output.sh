#!/usr/bin/env bash

# Set archive output variables for GitHub Actions.
#
# Required environment variables:
#   REF_NAME      - the git ref name (tag).
#   TARGET        - the build target (e.g. x86_64-pc-windows-msvc).
#   GITHUB_OUTPUT - path to GitHub Actions output file (set automatically).

set -xe -o pipefail

if [[ -z "${REF_NAME}" ]]; then
    echo "REF_NAME must be set" >&2
    exit 1
fi
if [[ -z "${TARGET}" ]]; then
    echo "TARGET must be set" >&2
    exit 1
fi
if [[ -z "${GITHUB_OUTPUT}" ]]; then
    echo "GITHUB_OUTPUT must be set" >&2
    exit 1
fi

prefix="${REF_NAME}-${TARGET}"

if [[ "${TARGET}" == *-pc-windows-msvc ]]; then
    echo "${TARGET}-tar=${prefix}.tar.gz" >> "${GITHUB_OUTPUT}"
    echo "${TARGET}-zip=${prefix}.zip" >> "${GITHUB_OUTPUT}"
else
    echo "${TARGET}-tar=${prefix}.tar.gz" >> "${GITHUB_OUTPUT}"
fi
