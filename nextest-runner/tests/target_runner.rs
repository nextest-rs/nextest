use camino::Utf8PathBuf;
use nextest_runner::{errors::TargetRunnerError, target_runner::TargetRunner};
use once_cell::sync::OnceCell;
use std::{env, sync::Mutex};

fn env_mutex() -> &'static Mutex<()> {
    static MUTEX: OnceCell<Mutex<()>> = OnceCell::new();
    MUTEX.get_or_init(|| Mutex::new(()))
}

pub fn with_env(
    vars: impl IntoIterator<Item = (impl Into<String>, impl AsRef<str>)>,
    func: impl FnOnce() -> Result<Option<TargetRunner>, TargetRunnerError>,
) -> Result<Option<TargetRunner>, TargetRunnerError> {
    let lock = env_mutex().lock().unwrap();

    let keys: Vec<_> = vars
        .into_iter()
        .map(|(key, val)| {
            let key = key.into();
            env::set_var(&key, val.as_ref());
            key
        })
        .collect();

    let res = func();

    for key in keys {
        env::remove_var(key);
    }
    drop(lock);

    res
}

fn default() -> &'static target_spec::Platform {
    static DEF: OnceCell<target_spec::Platform> = OnceCell::new();
    DEF.get_or_init(|| target_spec::Platform::current().unwrap())
}

#[test]
fn parses_cargo_env() {
    let def_runner = with_env(
        [(
            format!(
                "CARGO_TARGET_{}_RUNNER",
                default()
                    .triple_str()
                    .to_ascii_uppercase()
                    .replace('-', "_")
            ),
            "cargo_with_default --arg --arg2",
        )],
        || TargetRunner::for_target(None),
    )
    .unwrap()
    .unwrap();

    assert_eq!("cargo_with_default", def_runner.binary());
    assert_eq!(
        vec!["--arg", "--arg2"],
        def_runner.args().collect::<Vec<_>>()
    );

    let specific_runner = with_env(
        [(
            "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUNNER",
            "cargo_with_specific",
        )],
        || TargetRunner::for_target(Some("aarch64-linux-android")),
    )
    .unwrap()
    .unwrap();

    assert_eq!("cargo_with_specific", specific_runner.binary());
    assert_eq!(0, specific_runner.args().count());
}

/// Use fixtures/nextest-test as the root dir
fn root_dir() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fixtures/nextest-tests")
}

fn parse_triple(triple: &'static str) -> target_spec::Platform {
    target_spec::Platform::new(triple, target_spec::TargetFeatures::Unknown).unwrap()
}

#[test]
fn parses_cargo_config_exact() {
    let windows = parse_triple("x86_64-pc-windows-gnu");

    let runner = TargetRunner::find_config(windows, false, root_dir())
        .unwrap()
        .unwrap();

    assert_eq!("wine", runner.binary());
    assert_eq!(0, runner.args().count());
}

#[test]
fn disregards_non_matching() {
    let windows = parse_triple("x86_64-unknown-linux-gnu");
    assert!(TargetRunner::find_config(windows, false, root_dir())
        .unwrap()
        .is_none());
}

#[test]
fn parses_cargo_config_cfg() {
    let android = parse_triple("aarch64-linux-android");
    let runner = TargetRunner::find_config(android, false, root_dir())
        .unwrap()
        .unwrap();

    assert_eq!("android-runner", runner.binary());
    assert_eq!(vec!["-x"], runner.args().collect::<Vec<_>>());

    let linux = parse_triple("x86_64-unknown-linux-musl");
    let runner = TargetRunner::find_config(linux, false, root_dir())
        .unwrap()
        .unwrap();

    assert_eq!("passthrough", runner.binary());
    assert_eq!(
        vec!["--ensure-this-arg-is-sent"],
        runner.args().collect::<Vec<_>>()
    );
}

#[test]
fn fallsback_to_cargo_config() {
    let linux = parse_triple("x86_64-unknown-linux-musl");

    let runner = with_env(
        [(
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER",
            "cargo-runner-windows",
        )],
        || TargetRunner::with_root(Some(linux.triple_str()), false, root_dir()),
    )
    .unwrap()
    .unwrap();

    assert_eq!("passthrough", runner.binary());
    assert_eq!(
        vec!["--ensure-this-arg-is-sent"],
        runner.args().collect::<Vec<_>>()
    );
}
