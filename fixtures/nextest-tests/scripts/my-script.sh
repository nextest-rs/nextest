#!/bin/sh

# This depends only on /bin/sh, not bash, because some platforms like OpenBSD don't have bash
# installed by default.

# If this environment variable is set, exit with non-zero.
if [ -n "$__NEXTEST_SETUP_SCRIPT_ERROR" ]; then
    echo "__NEXTEST_SETUP_SCRIPT_ERROR is set, exiting with 1"
    exit 1
fi

echo MY_ENV_VAR=my-env-var >> "$NEXTEST_ENV"
