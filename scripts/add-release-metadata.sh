#!/usr/bin/env bash

# Add release metadata to the release-meta repository using mukti-bin.
#
# Required environment variables:
#   REF_NAME      - the git ref name (tag).
#   VERSION       - the release version.
#   ARCHIVE_*     - archive filenames for each target (see below).

set -xe -o pipefail

if [[ -z "${REF_NAME}" ]]; then
    echo "REF_NAME must be set" >&2
    exit 1
fi
if [[ -z "${VERSION}" ]]; then
    echo "VERSION must be set" >&2
    exit 1
fi

cd release-meta
~/bin/mukti-bin --json releases.json add-release \
    --version "${VERSION}" \
    --release-url "https://github.com/nextest-rs/nextest/releases/${REF_NAME}" \
    --archive-prefix "https://github.com/nextest-rs/nextest/releases/download/${REF_NAME}" \
    --archive "x86_64-unknown-linux-gnu:tar.gz=${ARCHIVE_X86_64_LINUX_TAR}" \
    --archive "x86_64-unknown-linux-musl:tar.gz=${ARCHIVE_X86_64_LINUX_MUSL_TAR}" \
    --archive "aarch64-unknown-linux-gnu:tar.gz=${ARCHIVE_AARCH64_LINUX_TAR}" \
    --archive "aarch64-unknown-linux-musl:tar.gz=${ARCHIVE_AARCH64_LINUX_MUSL_TAR}" \
    --archive "riscv64gc-unknown-linux-gnu:tar.gz=${ARCHIVE_RISCV64GC_LINUX_TAR}" \
    --archive "x86_64-pc-windows-msvc:tar.gz=${ARCHIVE_X86_64_WINDOWS_TAR}" \
    --archive "x86_64-pc-windows-msvc:zip=${ARCHIVE_X86_64_WINDOWS_ZIP}" \
    --archive "i686-pc-windows-msvc:tar.gz=${ARCHIVE_I686_WINDOWS_TAR}" \
    --archive "i686-pc-windows-msvc:zip=${ARCHIVE_I686_WINDOWS_ZIP}" \
    --archive "aarch64-pc-windows-msvc:tar.gz=${ARCHIVE_AARCH64_WINDOWS_TAR}" \
    --archive "aarch64-pc-windows-msvc:zip=${ARCHIVE_AARCH64_WINDOWS_ZIP}" \
    --archive "universal-apple-darwin:tar.gz=${ARCHIVE_MAC_TAR}" \
    --archive "x86_64-unknown-freebsd:tar.gz=${ARCHIVE_X86_64_FREEBSD_TAR}" \
    --archive "x86_64-unknown-illumos:tar.gz=${ARCHIVE_X86_64_ILLUMOS_TAR}"
