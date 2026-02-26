REM If this environment variable is set, exit with non-zero.
if defined __NEXTEST_SETUP_SCRIPT_ERROR exit 1

REM Check that NEXTEST_PROFILE is set.
if not defined NEXTEST_PROFILE exit 2

if defined CMD_ENV_VAR (
    ECHO SCRIPT_CMD_ENV_VAR=%CMD_ENV_VAR%>> %NEXTEST_ENV%
)
ECHO MY_ENV_VAR=my-env-var>> %NEXTEST_ENV%
ECHO SCRIPT_NEXTEST_PROFILE=%NEXTEST_PROFILE%>> %NEXTEST_ENV%

REM If this environment variable is set, write a NEXTEST-prefixed env var.
REM This is banned and should produce an error.
if defined __NEXTEST_SETUP_SCRIPT_RESERVED_ENV ECHO NEXTEST_BAD_VAR=bad-value>> %NEXTEST_ENV%
