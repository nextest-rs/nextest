// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(test)]
mod tests {
    #[test]
    fn test_out_dir_present() {
        // Since this package has a build script, ensure that OUT_DIR is correct by the presence of
        // the file that the script writes.
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is valid");
        let path = std::path::Path::new(&out_dir).join("this-is-a-test-file");
        let contents = std::fs::read(&path).expect("test file exists in OUT_DIR");
        assert_eq!(contents, b"test-contents");
    }

    #[test]
    fn test_build_script_vars_set() {
        // Since the build script wrote `cargo::rustc-env` instructions, these variables are
        // expected to be set by nextest
        #[cfg(new_format)]
        {
            let val = std::env::var("BUILD_SCRIPT_NEW_FMT").expect("BUILD_SCRIPT_NEW_FMT is valid");
            assert_eq!(val, "new_val");
        }
        let val = std::env::var("BUILD_SCRIPT_OLD_FMT").expect("BUILD_SCRIPT_OLD_FMT is valid");
        assert_eq!(val, "old_val");
    }
}
