REM If this environment variable is set, exit with non-zero.
if defined __NEXTEST_SETUP_SCRIPT_ERROR exit 1

REM Check that NEXTEST_PROFILE is set.
if not defined NEXTEST_PROFILE exit 2

if defined MY_ENV_VAR (
    ECHO MY_ENV_VAR=%MY_ENV_VAR%>> %NEXTEST_ENV%
    ECHO SCRIPT_NEXTEST=%NEXTEST%>> %NEXTEST_ENV%
) else (
    ECHO MY_ENV_VAR=my-env-var>> %NEXTEST_ENV%
)
ECHO SCRIPT_NEXTEST_PROFILE=%NEXTEST_PROFILE%>> %NEXTEST_ENV%
