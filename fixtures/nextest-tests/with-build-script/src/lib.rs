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
}
