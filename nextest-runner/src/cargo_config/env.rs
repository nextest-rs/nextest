// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use camino::Utf8Path;

/// Returns the directory against which relative paths are computed for the given config path.
pub fn relative_dir_for(config_path: &Utf8Path) -> Option<&Utf8Path> {
    // Need to call parent() twice here, since in Cargo land relative means relative to the *parent*
    // of the directory the config is in. First parent() gets the directory the config is in, and
    // the second one gets the parent of that.
    let relative_dir = config_path.parent()?.parent()?;

    // On Windows, remove the UNC prefix since Cargo does so as well.
    Some(strip_unc_prefix(relative_dir))
}

#[cfg(windows)]
#[inline]
fn strip_unc_prefix(path: &Utf8Path) -> &Utf8Path {
    dunce::simplified(path.as_std_path())
        .try_into()
        .expect("stripping verbatim components from a UTF-8 path should result in a UTF-8 path")
}

#[cfg(not(windows))]
#[inline]
fn strip_unc_prefix(path: &Utf8Path) -> &Utf8Path {
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_config::{test_helpers::setup_temp_dir, CargoConfigs};
    use camino::Utf8PathBuf;

    #[test]
    fn test_env_var_precedence() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        let configs =
            CargoConfigs::new_with_isolation(&[] as &[&str], &dir_foo_bar_path, &dir_path).unwrap();
        let env = configs.env();
        let env_values: Vec<&str> = env.iter().map(|elem| elem.value.as_str()).collect();
        assert_eq!(env_values, vec!["foo-bar-config", "foo-config"]);

        let configs = CargoConfigs::new_with_isolation(
            &["env.SOME_VAR=\"cli-config\""],
            &dir_foo_bar_path,
            &dir_path,
        )
        .unwrap();
        let env = configs.env();
        let env_values: Vec<&str> = env.iter().map(|elem| elem.value.as_str()).collect();
        assert_eq!(
            env_values,
            vec!["cli-config", "foo-bar-config", "foo-config"]
        );
    }

    #[test]
    fn test_cli_env_var_relative() {
        let dir = setup_temp_dir().unwrap();
        let dir_path = Utf8PathBuf::try_from(dir.path().canonicalize().unwrap()).unwrap();
        let dir_foo_path = dir_path.join("foo");
        let dir_foo_bar_path = dir_foo_path.join("bar");

        CargoConfigs::new_with_isolation(
            &["env.SOME_VAR={value = \"path\", relative = true }"],
            &dir_foo_bar_path,
            &dir_path,
        )
        .expect_err("CLI configs can't be relative");

        CargoConfigs::new_with_isolation(
            &["env.SOME_VAR.value=\"path\"", "env.SOME_VAR.relative=true"],
            &dir_foo_bar_path,
            &dir_path,
        )
        .expect_err("CLI configs can't be relative");
    }

    #[test]
    #[cfg(unix)]
    fn test_relative_dir_for_unix() {
        assert_eq!(
            relative_dir_for("/foo/bar/.cargo/config.toml".as_ref()),
            Some("/foo/bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("/foo/bar/.cargo/config".as_ref()),
            Some("/foo/bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("/foo/bar/config".as_ref()),
            Some("/foo".as_ref())
        );
        assert_eq!(relative_dir_for("/foo/config".as_ref()), Some("/".as_ref()));
        assert_eq!(relative_dir_for("/config.toml".as_ref()), None);
    }

    #[test]
    #[cfg(windows)]
    fn test_relative_dir_for_windows() {
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\.cargo\\config.toml".as_ref()),
            Some("C:\\foo\\bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\.cargo\\config".as_ref()),
            Some("C:\\foo\\bar".as_ref()),
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\bar\\config".as_ref()),
            Some("C:\\foo".as_ref())
        );
        assert_eq!(
            relative_dir_for("C:\\foo\\config".as_ref()),
            Some("C:\\".as_ref())
        );
        assert_eq!(relative_dir_for("C:\\config.toml".as_ref()), None);
    }
}
