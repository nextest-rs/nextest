// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform-specific state directory discovery for nextest records.
//!
//! Test run recordings are stored in `XDG_STATE_HOME` (on Linux/macOS) rather
//! than `XDG_CACHE_HOME` because they are accumulated state, not regenerable
//! cache data. The XDG spec defines cache as "non-essential data files [that]
//! the application must be able to regenerate," but recordings capture a
//! specific execution at a specific point in time and cannot be regenerated.

use crate::errors::{DisplayErrorChain, StateDirError};
use camino::{Utf8Path, Utf8PathBuf};
use etcetera::{BaseStrategy, choose_base_strategy};
use std::fs;
use tracing::{info, warn};
use xxhash_rust::xxh3::xxh3_64;

/// Maximum length of the encoded workspace path in bytes.
const MAX_ENCODED_LEN: usize = 96;

/// Length of the hash suffix appended to truncated paths.
///
/// Between the first many bytes and this, we should ideally have more than
/// enough entropy to disambiguate repos.
const HASH_SUFFIX_LEN: usize = 8;

/// Environment variable to override the nextest state directory.
///
/// When set, this overrides the platform-specific state directory. The records
/// directory will be `$NEXTEST_STATE_DIR/projects/<encoded-workspace>/records/`.
pub const NEXTEST_STATE_DIR_ENV: &str = "NEXTEST_STATE_DIR";

/// Returns the platform-specific state directory for nextest records for a workspace.
///
/// If the `NEXTEST_STATE_DIR` environment variable is set, uses that as the base
/// directory. Otherwise, uses the platform-specific default:
///
/// - Linux, macOS, and other Unix: `$XDG_STATE_HOME/nextest/projects/<encoded-workspace>/records/`
///   or `~/.local/state/nextest/projects/<encoded-workspace>/records/`
/// - Windows: `%LOCALAPPDATA%\nextest\projects\<encoded-workspace>\records\`
///   (Windows has no state directory concept, so falls back to cache directory.)
///
/// The workspace root is canonicalized (symlinks resolved) before being encoded
/// using `encode_workspace_path` to produce a directory-safe, bijective
/// representation. This ensures that accessing a workspace via a symlink
/// produces the same state directory as accessing it via the real path.
///
/// ## Migration from cache to state directory
///
/// On first access after upgrading, this function automatically migrates the
/// entire `~/.cache/nextest/` directory (nextest <= 0.9.125) to
/// `~/.local/state/nextest/` if the old location exists and the new one does
/// not. This is a one-time migration.
///
/// Returns an error if:
///
/// - The platform state directory cannot be determined.
/// - The workspace path cannot be canonicalized (e.g., doesn't exist).
/// - Any path is not valid UTF-8.
pub fn records_state_dir(workspace_root: &Utf8Path) -> Result<Utf8PathBuf, StateDirError> {
    // If NEXTEST_STATE_DIR is set, use it directly (no migration).
    if let Ok(state_dir) = std::env::var(NEXTEST_STATE_DIR_ENV) {
        let base_dir = Utf8PathBuf::from(state_dir);
        let canonical_workspace =
            workspace_root
                .canonicalize_utf8()
                .map_err(|error| StateDirError::Canonicalize {
                    workspace_root: workspace_root.to_owned(),
                    error,
                })?;
        let encoded_workspace = encode_workspace_path(&canonical_workspace);
        return Ok(base_dir
            .join("projects")
            .join(&encoded_workspace)
            .join("records"));
    }

    let strategy = choose_base_strategy().map_err(StateDirError::BaseDirStrategy)?;

    // Canonicalize the workspace root to resolve symlinks. This ensures that
    // accessing a workspace via a symlink produces the same state directory.
    let canonical_workspace =
        workspace_root
            .canonicalize_utf8()
            .map_err(|error| StateDirError::Canonicalize {
                workspace_root: workspace_root.to_owned(),
                error,
            })?;
    let encoded_workspace = encode_workspace_path(&canonical_workspace);

    // Compute the state directory path. Use state_dir() if available, otherwise
    // fall back to cache_dir() (Windows has no state directory concept).
    let nextest_dir = if let Some(base_state_dir) = strategy.state_dir() {
        // The state directory is available (Unix with XDG). Attempt a one-time
        // migration from the old cache location.
        let nextest_state = base_state_dir.join("nextest");
        let nextest_cache = strategy.cache_dir().join("nextest");
        if let (Ok(nextest_state_utf8), Ok(nextest_cache_utf8)) = (
            Utf8PathBuf::from_path_buf(nextest_state.clone()),
            Utf8PathBuf::from_path_buf(nextest_cache),
        ) && nextest_state_utf8 != nextest_cache_utf8
        {
            migrate_nextest_dir(&nextest_cache_utf8, &nextest_state_utf8);
        };
        nextest_state
    } else {
        // No state directory (Windows). Use cache directory directly.
        strategy.cache_dir().join("nextest")
    };

    let nextest_dir_utf8 = Utf8PathBuf::from_path_buf(nextest_dir.clone())
        .map_err(|_| StateDirError::StateDirNotUtf8 { path: nextest_dir })?;

    Ok(nextest_dir_utf8
        .join("projects")
        .join(&encoded_workspace)
        .join("records"))
}

/// Attempts to migrate the entire nextest directory from cache to state location.
///
/// This is a one-time migration.
fn migrate_nextest_dir(old_dir: &Utf8Path, new_dir: &Utf8Path) {
    if !old_dir.exists() || new_dir.exists() {
        return;
    }

    if let Some(parent) = new_dir.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        warn!(
            "failed to create parent directory for new state location \
             at `{new_dir}`: {}",
            DisplayErrorChain::new(&error),
        );
        return;
    }

    // Attempt an atomic rename.
    match fs::rename(old_dir, new_dir) {
        Ok(()) => {
            info!("migrated nextest recordings from `{old_dir}` to `{new_dir}`");
        }
        Err(error) => {
            warn!(
                "failed to migrate nextest recordings from `{old_dir}` to `{new_dir}` \
                 (cross-filesystem move or permission issue): {}",
                DisplayErrorChain::new(&error),
            );
        }
    }
}

/// Encodes a workspace path into a directory-safe string.
///
/// The encoding is bijective (reversible) and produces valid directory names on all
/// platforms. The encoding scheme uses underscore as an escape character:
///
/// - `_` → `__` (escape underscore first)
/// - `/` → `_s` (Unix path separator)
/// - `\` → `_b` (Windows path separator)
/// - `:` → `_c` (Windows drive letter separator)
/// - `*` → `_a` (asterisk, invalid on Windows)
/// - `"` → `_q` (double quote, invalid on Windows)
/// - `<` → `_l` (less than, invalid on Windows)
/// - `>` → `_g` (greater than, invalid on Windows)
/// - `|` → `_p` (pipe, invalid on Windows)
/// - `?` → `_m` (question mark, invalid on Windows)
///
/// If the encoded path exceeds 96 bytes, it is truncated at a valid UTF-8 boundary
/// and an 8-character hash suffix is appended to maintain uniqueness.
///
/// # Examples
///
/// - `/home/rain/dev/nextest` → `_shome_srain_sdev_snextest`
/// - `C:\Users\rain\dev` → `C_c_bUsers_brain_bdev`
/// - `/path_with_underscore` → `_spath__with__underscore`
/// - `/weird*path?` → `_sweird_apath_m`
pub fn encode_workspace_path(path: &Utf8Path) -> String {
    let mut encoded = String::with_capacity(path.as_str().len() * 2);

    for ch in path.as_str().chars() {
        match ch {
            '_' => encoded.push_str("__"),
            '/' => encoded.push_str("_s"),
            '\\' => encoded.push_str("_b"),
            ':' => encoded.push_str("_c"),
            '*' => encoded.push_str("_a"),
            '"' => encoded.push_str("_q"),
            '<' => encoded.push_str("_l"),
            '>' => encoded.push_str("_g"),
            '|' => encoded.push_str("_p"),
            '?' => encoded.push_str("_m"),
            _ => encoded.push(ch),
        }
    }

    truncate_with_hash(encoded)
}

/// Truncates an encoded string to fit within [`MAX_ENCODED_LEN`] bytes.
///
/// If the string is already short enough, returns it unchanged. Otherwise,
/// truncates at a valid UTF-8 boundary and appends an 8-character hash suffix
/// derived from the full string.
fn truncate_with_hash(encoded: String) -> String {
    if encoded.len() <= MAX_ENCODED_LEN {
        return encoded;
    }

    // Compute hash of full string before truncation.
    let hash = xxh3_64(encoded.as_bytes());
    let hash_suffix = format!("{:08x}", hash & 0xFFFFFFFF);

    // Find the longest valid UTF-8 prefix that fits.
    let max_prefix_len = MAX_ENCODED_LEN - HASH_SUFFIX_LEN;
    let bytes = encoded.as_bytes();
    let truncated_bytes = &bytes[..max_prefix_len.min(bytes.len())];

    // Use utf8_chunks to find the valid UTF-8 portion.
    let mut valid_len = 0;
    for chunk in truncated_bytes.utf8_chunks() {
        valid_len += chunk.valid().len();
        // Stop at first invalid sequence (which would be an incomplete multi-byte char).
        if !chunk.invalid().is_empty() {
            break;
        }
    }

    let mut result = encoded[..valid_len].to_string();
    result.push_str(&hash_suffix);
    result
}

/// Decodes a workspace path that was encoded with [`encode_workspace_path`].
///
/// Returns `None` if the encoded string is malformed (contains an invalid escape
/// sequence like `_x` where `x` is not a recognized escape character).
#[cfg_attr(not(test), expect(dead_code))] // Will be used in replay phase.
pub fn decode_workspace_path(encoded: &str) -> Option<Utf8PathBuf> {
    let mut decoded = String::with_capacity(encoded.len());
    let mut chars = encoded.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '_' {
            match chars.next() {
                Some('_') => decoded.push('_'),
                Some('s') => decoded.push('/'),
                Some('b') => decoded.push('\\'),
                Some('c') => decoded.push(':'),
                Some('a') => decoded.push('*'),
                Some('q') => decoded.push('"'),
                Some('l') => decoded.push('<'),
                Some('g') => decoded.push('>'),
                Some('p') => decoded.push('|'),
                Some('m') => decoded.push('?'),
                // Malformed: `_` at end of string or followed by unknown char.
                _ => return None,
            }
        } else {
            decoded.push(ch);
        }
    }

    Some(Utf8PathBuf::from(decoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_records_state_dir() {
        // Use a real existing path (the temp dir always exists).
        let temp_dir =
            Utf8PathBuf::try_from(std::env::temp_dir()).expect("temp dir should be valid UTF-8");
        let state_dir = records_state_dir(&temp_dir).expect("state directory should be available");

        assert!(
            state_dir.as_str().contains("nextest"),
            "state dir should contain 'nextest': {state_dir}"
        );
        assert!(
            state_dir.as_str().contains("projects"),
            "state dir should contain 'projects': {state_dir}"
        );
        assert!(
            state_dir.as_str().contains("records"),
            "state dir should contain 'records': {state_dir}"
        );
    }

    #[test]
    fn test_records_state_dir_canonicalizes_symlinks() {
        // Create a temp directory and a symlink pointing to it.
        let temp_dir = camino_tempfile::tempdir().expect("tempdir should be created");
        let real_path = temp_dir.path().to_path_buf();

        // Create a subdirectory to serve as the "workspace".
        let workspace = real_path.join("workspace");
        fs::create_dir(&workspace).expect("workspace dir should be created");

        // Create a symlink pointing to the workspace.
        let symlink_path = real_path.join("symlink-to-workspace");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&workspace, &symlink_path)
            .expect("symlink should be created on Unix");

        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&workspace, &symlink_path)
            .expect("symlink should be created on Windows");

        // Get state dir via the real path.
        let state_via_real =
            records_state_dir(&workspace).expect("state dir via real path should be available");

        // Get state dir via the symlink.
        let state_via_symlink =
            records_state_dir(&symlink_path).expect("state dir via symlink should be available");

        // They should be the same because canonicalization resolves the symlink.
        assert_eq!(
            state_via_real, state_via_symlink,
            "state dir should be the same whether accessed via real path or symlink"
        );
    }

    #[test]
    fn test_migration_from_cache_to_state() {
        // This test verifies that the entire nextest directory is migrated at once.
        let temp_dir = camino_tempfile::tempdir().expect("tempdir should be created");
        let base = temp_dir.path();

        // Create the old cache nextest directory with multiple workspaces.
        let old_nextest = base.join("cache").join("nextest");
        let workspace1_records = old_nextest
            .join("projects")
            .join("workspace1")
            .join("records");
        let workspace2_records = old_nextest
            .join("projects")
            .join("workspace2")
            .join("records");
        fs::create_dir_all(&workspace1_records).expect("workspace1 dir should be created");
        fs::create_dir_all(&workspace2_records).expect("workspace2 dir should be created");

        // Create marker files in both workspaces.
        fs::write(workspace1_records.join("runs.json.zst"), b"workspace1 data")
            .expect("workspace1 marker should be created");
        fs::write(workspace2_records.join("runs.json.zst"), b"workspace2 data")
            .expect("workspace2 marker should be created");

        // Verify the old location exists.
        assert!(
            old_nextest.exists(),
            "old nextest dir should exist before migration"
        );

        // Simulate migration by calling migrate_nextest_dir directly.
        let new_nextest = base.join("state").join("nextest");
        migrate_nextest_dir(&old_nextest, &new_nextest);

        // Verify migration succeeded: old is gone, new has all the content.
        assert!(
            !old_nextest.exists(),
            "old nextest dir should not exist after migration"
        );
        assert!(
            new_nextest.exists(),
            "new nextest dir should exist after migration"
        );
        assert!(
            new_nextest
                .join("projects")
                .join("workspace1")
                .join("records")
                .join("runs.json.zst")
                .exists(),
            "workspace1 marker should exist in new location"
        );
        assert!(
            new_nextest
                .join("projects")
                .join("workspace2")
                .join("records")
                .join("runs.json.zst")
                .exists(),
            "workspace2 marker should exist in new location"
        );
    }

    #[test]
    fn test_migration_skipped_if_new_exists() {
        let temp_dir = camino_tempfile::tempdir().expect("tempdir should be created");
        let base = temp_dir.path();

        let old_nextest = base.join("cache").join("nextest");
        let new_nextest = base.join("state").join("nextest");
        fs::create_dir_all(old_nextest.join("projects")).expect("old dir should be created");
        fs::create_dir_all(new_nextest.join("projects")).expect("new dir should be created");

        // Put different content in each to verify no migration occurs.
        fs::write(old_nextest.join("old_marker"), b"old").expect("old marker should be created");
        fs::write(new_nextest.join("new_marker"), b"new").expect("new marker should be created");

        migrate_nextest_dir(&old_nextest, &new_nextest);

        // Both should still exist with their original content.
        assert!(old_nextest.exists(), "old dir should still exist");
        assert!(new_nextest.exists(), "new dir should still exist");
        assert!(
            old_nextest.join("old_marker").exists(),
            "old marker should still exist"
        );
        assert!(
            new_nextest.join("new_marker").exists(),
            "new marker should still exist"
        );
    }

    #[test]
    fn test_migration_skipped_if_old_does_not_exist() {
        // Migration should not occur if the old directory doesn't exist.
        let temp_dir = camino_tempfile::tempdir().expect("tempdir should be created");
        let base = temp_dir.path();

        let old_nextest = base.join("cache").join("nextest");
        let new_nextest = base.join("state").join("nextest");

        assert!(!old_nextest.exists());
        assert!(!new_nextest.exists());

        migrate_nextest_dir(&old_nextest, &new_nextest);

        assert!(!old_nextest.exists());
        assert!(!new_nextest.exists());
    }

    // Basic encoding tests.
    #[test]
    fn test_encode_workspace_path() {
        let cases = [
            ("", ""),
            ("simple", "simple"),
            ("/home/user", "_shome_suser"),
            ("/home/user/project", "_shome_suser_sproject"),
            ("C:\\Users\\name", "C_c_bUsers_bname"),
            ("D:\\dev\\project", "D_c_bdev_bproject"),
            ("/path_with_underscore", "_spath__with__underscore"),
            ("C:\\path_name", "C_c_bpath__name"),
            ("/a/b/c", "_sa_sb_sc"),
            // Windows-invalid characters.
            ("/weird*path", "_sweird_apath"),
            ("/path?query", "_spath_mquery"),
            ("/file<name>", "_sfile_lname_g"),
            ("/path|pipe", "_spath_ppipe"),
            ("/\"quoted\"", "_s_qquoted_q"),
            // All Windows-invalid characters combined.
            ("*\"<>|?", "_a_q_l_g_p_m"),
        ];

        for (input, expected) in cases {
            let encoded = encode_workspace_path(Utf8Path::new(input));
            assert_eq!(
                encoded, expected,
                "encoding failed for {input:?}: expected {expected:?}, got {encoded:?}"
            );
        }
    }

    // Roundtrip tests: encode then decode should return original.
    #[test]
    fn test_encode_decode_roundtrip() {
        let cases = [
            "/home/user/project",
            "C:\\Users\\name\\dev",
            "/path_with_underscore",
            "/_",
            "_/",
            "__",
            "/a_b/c_d",
            "",
            "no_special_chars",
            "/mixed\\path:style",
            // Windows-invalid characters (valid on Unix).
            "/path*with*asterisks",
            "/file?query",
            "/path<with>angles",
            "/pipe|char",
            "/\"quoted\"",
            // All special chars in one path.
            "/all*special?chars<in>one|path\"here\"_end",
        ];

        for original in cases {
            let encoded = encode_workspace_path(Utf8Path::new(original));
            let decoded = decode_workspace_path(&encoded);
            assert_eq!(
                decoded.as_deref(),
                Some(Utf8Path::new(original)),
                "roundtrip failed for {original:?}: encoded={encoded:?}, decoded={decoded:?}"
            );
        }
    }

    // Bijectivity tests: different inputs must produce different outputs.
    #[test]
    fn test_encoding_is_bijective() {
        // These pairs were problematic with the simple dash-based encoding.
        let pairs = [
            ("/-", "-/"),
            ("/a", "_a"),
            ("_s", "/"),
            ("a_", "a/"),
            ("__", "_"),
            ("/", "\\"),
            // New escape sequences for Windows-invalid characters.
            ("_a", "*"),
            ("_q", "\""),
            ("_l", "<"),
            ("_g", ">"),
            ("_p", "|"),
            ("_m", "?"),
            // Ensure Windows-invalid chars don't collide with each other.
            ("*", "?"),
            ("<", ">"),
            ("|", "\""),
        ];

        for (a, b) in pairs {
            let encoded_a = encode_workspace_path(Utf8Path::new(a));
            let encoded_b = encode_workspace_path(Utf8Path::new(b));
            assert_ne!(
                encoded_a, encoded_b,
                "bijectivity violated: {a:?} and {b:?} both encode to {encoded_a:?}"
            );
        }
    }

    // Decode should reject malformed inputs.
    #[test]
    fn test_decode_rejects_malformed() {
        let malformed_inputs = [
            "_",     // underscore at end
            "_x",    // unknown escape sequence
            "foo_",  // underscore at end after content
            "foo_x", // unknown escape in middle
            "_S",    // uppercase S not valid
        ];

        for input in malformed_inputs {
            assert!(
                decode_workspace_path(input).is_none(),
                "should reject malformed input: {input:?}"
            );
        }
    }

    // Valid escape sequences should decode.
    #[test]
    fn test_decode_valid_escapes() {
        let cases = [
            ("__", "_"),
            ("_s", "/"),
            ("_b", "\\"),
            ("_c", ":"),
            ("a__b", "a_b"),
            ("_shome", "/home"),
            // Windows-invalid character escapes.
            ("_a", "*"),
            ("_q", "\""),
            ("_l", "<"),
            ("_g", ">"),
            ("_p", "|"),
            ("_m", "?"),
            // Combined.
            ("_spath_astar_mquery", "/path*star?query"),
        ];

        for (input, expected) in cases {
            let decoded = decode_workspace_path(input);
            assert_eq!(
                decoded.as_deref(),
                Some(Utf8Path::new(expected)),
                "decode failed for {input:?}: expected {expected:?}, got {decoded:?}"
            );
        }
    }

    // Truncation tests.
    #[test]
    fn test_short_paths_not_truncated() {
        // A path that encodes to exactly 96 bytes should not be truncated.
        let short_path = "/a/b/c/d";
        let encoded = encode_workspace_path(Utf8Path::new(short_path));
        assert!(
            encoded.len() <= MAX_ENCODED_LEN,
            "short path should not be truncated: {encoded:?} (len={})",
            encoded.len()
        );
        // Should not contain a hash suffix (no truncation occurred).
        assert_eq!(encoded, "_sa_sb_sc_sd");
    }

    #[test]
    fn test_long_paths_truncated_with_hash() {
        // Create a path that will definitely exceed 96 bytes when encoded.
        // Each `/x` becomes `_sx` (3 bytes), so we need > 32 components.
        let long_path = "/a".repeat(50); // 100 bytes raw, 150 bytes encoded
        let encoded = encode_workspace_path(Utf8Path::new(&long_path));

        assert_eq!(
            encoded.len(),
            MAX_ENCODED_LEN,
            "truncated path should be exactly {MAX_ENCODED_LEN} bytes: {encoded:?} (len={})",
            encoded.len()
        );

        // Should end with an 8-character hex hash.
        let hash_suffix = &encoded[encoded.len() - HASH_SUFFIX_LEN..];
        assert!(
            hash_suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "hash suffix should be hex digits: {hash_suffix:?}"
        );
    }

    #[test]
    fn test_truncation_preserves_uniqueness() {
        // Two different long paths should produce different truncated results.
        let path_a = "/a".repeat(50);
        let path_b = "/b".repeat(50);

        let encoded_a = encode_workspace_path(Utf8Path::new(&path_a));
        let encoded_b = encode_workspace_path(Utf8Path::new(&path_b));

        assert_ne!(
            encoded_a, encoded_b,
            "different paths should produce different encodings even when truncated"
        );
    }

    #[test]
    fn test_truncation_with_unicode() {
        // Create a path with multi-byte UTF-8 characters that would be split.
        // '日' is 3 bytes in UTF-8.
        let unicode_path = "/日本語".repeat(20); // Each repeat is 10 bytes raw.
        let encoded = encode_workspace_path(Utf8Path::new(&unicode_path));

        assert!(
            encoded.len() <= MAX_ENCODED_LEN,
            "encoded path should not exceed {MAX_ENCODED_LEN} bytes: len={}",
            encoded.len()
        );

        // Verify the result is valid UTF-8 (this would panic if not).
        let _ = encoded.as_str();

        // Verify the hash suffix is present and valid hex.
        let hash_suffix = &encoded[encoded.len() - HASH_SUFFIX_LEN..];
        assert!(
            hash_suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "hash suffix should be hex digits: {hash_suffix:?}"
        );
    }

    #[test]
    fn test_truncation_boundary_at_96_bytes() {
        // Create paths of varying lengths around the 96-byte boundary.
        // The encoding doubles some characters, so we need to be careful.

        // A path that encodes to exactly 96 bytes should not be truncated.
        // 'a' stays as 'a', so we can use a string of 96 'a's.
        let exactly_96 = "a".repeat(96);
        let encoded = encode_workspace_path(Utf8Path::new(&exactly_96));
        assert_eq!(encoded.len(), 96);
        assert_eq!(encoded, exactly_96); // No hash suffix.

        // A path that encodes to 97 bytes should be truncated.
        let just_over = "a".repeat(97);
        let encoded = encode_workspace_path(Utf8Path::new(&just_over));
        assert_eq!(encoded.len(), 96);
        // Should have hash suffix.
        let hash_suffix = &encoded[90..];
        assert!(hash_suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_truncation_different_suffixes_same_prefix() {
        // Two paths with the same prefix but different endings should get different hashes.
        let base = "a".repeat(90);
        let path_a = format!("{base}XXXXXXX");
        let path_b = format!("{base}YYYYYYY");

        let encoded_a = encode_workspace_path(Utf8Path::new(&path_a));
        let encoded_b = encode_workspace_path(Utf8Path::new(&path_b));

        // Both should be truncated (97 chars each).
        assert_eq!(encoded_a.len(), 96);
        assert_eq!(encoded_b.len(), 96);

        // The hash suffixes should be different.
        assert_ne!(
            &encoded_a[90..],
            &encoded_b[90..],
            "different paths should have different hash suffixes"
        );
    }
}
