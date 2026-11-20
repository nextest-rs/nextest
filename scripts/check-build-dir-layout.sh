#!/usr/bin/env bash

# Checks that a Cargo build directory uses the expected layout.
#
# The build-dir CI job selects the layout implicitly. To avoid the layout
# changing out from underneath us, we also do an explicit check.
#
# Usage: check-build-dir-layout.sh <new|legacy> <build-dir>

set -e

usage="usage: $0 <new|legacy> <build-dir>"
if [[ $# -ne 2 ]]; then
    echo "$usage" >&2
    exit 2
fi

expected="$1"
build_dir="$2"
profile_dir="$build_dir/debug"

case "$expected" in
    new)
        description="debug/build/<pkg>/<hash>/fingerprint, and no debug/deps or debug/.fingerprint"
        hint="Cargo may have changed its default layout. If so, pass -Zbuild-dir-new-layout to this step to select the new layout explicitly."
        ;;
    legacy)
        description="debug/deps and debug/.fingerprint, and no debug/build/<pkg>/<hash>/fingerprint"
        hint="Cargo may have removed __CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT. If so, run this step with a toolchain older than Rust 1.100."
        ;;
    *)
        echo "error: unknown layout '$expected' (expected 'new' or 'legacy')" >&2
        echo "$usage" >&2
        exit 2
        ;;
esac

if [[ ! -d "$profile_dir" ]]; then
    echo "error: $profile_dir does not exist, so the build directory layout can't be checked" >&2
    echo "(did the build use a different build directory or profile?)" >&2
    exit 1
fi

shopt -s nullglob
new_layout_fingerprints=("$profile_dir"/build/*/*/fingerprint)

has_new=false
if [[ ${#new_layout_fingerprints[@]} -gt 0 ]]; then
    has_new=true
fi
has_legacy=false
if [[ -d "$profile_dir/deps" && -d "$profile_dir/.fingerprint" ]]; then
    has_legacy=true
fi
has_any_legacy=false
if [[ -e "$profile_dir/deps" || -e "$profile_dir/.fingerprint" ]]; then
    has_any_legacy=true
fi

if [[ "$has_new" == true && "$has_any_legacy" == false ]]; then
    actual=new found="the new layout"
elif [[ "$has_new" == false && "$has_legacy" == true ]]; then
    actual=legacy found="the legacy layout"
elif [[ "$has_new" == false && "$has_any_legacy" == false ]]; then
    actual=unknown found="neither layout"
elif [[ "$has_new" == true ]]; then
    actual=unknown found="a mix of both layouts"
else
    actual=unknown found="an incomplete legacy layout (only one of debug/deps and debug/.fingerprint)"
fi

if [[ "$actual" != "$expected" ]]; then
    echo "error: expected $build_dir to use the $expected build directory layout, but found $found" >&2
    echo "(the $expected layout has $description)" >&2
    echo "$hint" >&2
    exit 1
fi

echo "$build_dir uses the $expected build directory layout"
