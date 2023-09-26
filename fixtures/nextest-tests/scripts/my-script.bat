REM If this environment variable is set, exit with non-zero.
if defined __NEXTEST_SETUP_SCRIPT_ERROR exit 1

ECHO MY_ENV_VAR=my-env-var>> %NEXTEST_ENV%
