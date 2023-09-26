#!/bin/bash

# If this environment variable is set, exit with non-zero.
if [ ! -z "$__NEXTEST_SETUP_SCRIPT_ERROR" ]; then
    echo "__NEXTEST_SETUP_SCRIPT_ERROR is set, exiting with 1"
    exit 1
fi

echo MY_ENV_VAR=my-env-var >> $NEXTEST_ENV
