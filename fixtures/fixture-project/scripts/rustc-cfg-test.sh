# Copyright (c) The nextest Contributors
# SPDX-License-Identifier: MIT OR Apache-2.0

#!/bin/sh

if [ "$1" = "--print=cfg" ]; then
    cat <<EOF
debug_assertions
panic="abort"
target_abi="eabihf"
target_arch="arm"
target_endian="little"
target_env="musl"
target_family="unix"
target_has_atomic="16"
target_has_atomic="32"
target_has_atomic="8"
target_has_atomic="ptr"
target_os="linux"
target_pointer_width="32"
target_vendor="unknown"
unix
EOF
else
    exec rustc "$@"
fi
