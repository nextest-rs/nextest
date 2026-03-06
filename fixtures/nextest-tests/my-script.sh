#!/bin/sh

# Notes:
#
# 1. We put this file in here rather than in the `scripts` subdirectory because
#    we want to specify it as "my-script.sh" -- this enables testing against
#    "workspace-root".
#
# 2. This depends only on /bin/sh, not bash, because some platforms like
#    OpenBSD don't have bash installed by default.

# If this environment variable is set, exit with non-zero.
if [ -n "$__NEXTEST_SETUP_SCRIPT_ERROR" ]; then
    echo "__NEXTEST_SETUP_SCRIPT_ERROR is set, exiting with 1"
    exit 1
fi

# If NEXTEST_PROFILE is not set, exit with non-zero.
if [ -z "$NEXTEST_PROFILE" ]; then
    echo "NEXTEST_PROFILE is not set, exiting with 2"
    exit 2
fi

if [ -n "$CMD_ENV_VAR" ]; then
    echo SCRIPT_CMD_ENV_VAR="$CMD_ENV_VAR" >> "$NEXTEST_ENV"
fi
echo MY_ENV_VAR=my-env-var >> "$NEXTEST_ENV"
echo SCRIPT_NEXTEST_PROFILE="$NEXTEST_PROFILE" >> "$NEXTEST_ENV"

# If this environment variable is set, write a NEXTEST-prefixed env var to
# NEXTEST_ENV. This is banned and should produce an error.
if [ -n "$__NEXTEST_SETUP_SCRIPT_RESERVED_ENV" ]; then
    echo "NEXTEST_BAD_VAR=bad-value" >> "$NEXTEST_ENV"
fi
