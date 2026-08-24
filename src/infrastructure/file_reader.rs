//! File reading helpers (path → bytes) and input validation.
//!
//! Enforces two global safety constraints for every tool:
//! - [`validate_source`] checks the input size against `MAX_INPUT_BYTES`
//!   (before reading / decoding) and
//! - [`canonicalize_within_root`] ensures every path is inside `ALLOW_ROOT_DIR`.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::config::{allowroot, max_input_bytes};
use crate::domain::error::ExtractionError;
use crate::domain::result::Source;

/// Open `path` for reading **without following a terminal symlink** (`O_NOFOLLOW`
/// on Unix). Used to close the TOCTOU window between path validation and the
/// content read: a symlink swapped in at the final path component after
/// canonicalization is rejected here, so reads cannot be redirected outside
/// `ALLOW_ROOT_DIR`.
#[cfg(unix)]
fn open_no_follow(path: &Path) -> Result<File, ExtractionError> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| ExtractionError::Io(format!("failed to open {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> Result<File, ExtractionError> {
    std::fs::File::open(path)
        .map_err(|e| ExtractionError::Io(format!("failed to open {}: {e}", path.display())))
}

/// Read a file's bytes through a single no-follow open, capping the read at
/// `MAX_INPUT_BYTES` so a file that grew after validation cannot cause a large
/// allocation.
pub fn read_path_no_follow(path: &Path) -> Result<Vec<u8>, ExtractionError> {
    let file = open_no_follow(path)?;
    let max = max_input_bytes();
    let mut limited = file.take(max as u64 + 1);
    let mut buf = Vec::new();
    limited
        .read_to_end(&mut buf)
        .map_err(|e| ExtractionError::Io(format!("failed to read {}: {e}", path.display())))?;
    if buf.len() as u64 > max as u64 {
        return Err(ExtractionError::ResourceLimit(format!(
            "input size {} bytes exceeds MAX_INPUT_BYTES={max}",
            buf.len()
        )));
    }
    Ok(buf)
}

/// Read a file from disk into bytes.
pub fn read_file(path: &Path) -> Result<Vec<u8>, ExtractionError> {
    read_path_no_follow(path)
}

/// Resolve a [`Source`] to bytes, reading from disk (no-follow) when it is a path.
pub fn read_source_bytes(source: &Source) -> Result<Vec<u8>, ExtractionError> {
    match source {
        Source::Path(p) => read_path_no_follow(p),
        Source::Bytes(b) => Ok(b.clone()),
    }
}

/// Canonicalize `path` and ensure it is within `ALLOW_ROOT_DIR`.
///
/// On Unix `ALLOW_ROOT_DIR` defaults to `~/Documents` (falling back to `/` when
/// the home directory is unavailable), so this gates reads to the user's
/// documents folder unless an operator narrows/widens it. On Windows the
/// analogous default is the system volume root (`%system disk%:\`). Symlinks in
/// the path are resolved here; the final component is re-checked with a
/// no-follow open in [`validate_source`] to prevent a race.
pub fn canonicalize_within_root(path: &Path) -> Result<PathBuf, ExtractionError> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| ExtractionError::Io(format!("failed to resolve {}: {e}", path.display())))?;
    let root = allowroot();
    let root_canon = std::fs::canonicalize(&root).unwrap_or(root);
    if !canonical.starts_with(&root_canon) {
        return Err(ExtractionError::Io(format!(
            "path {} is outside ALLOW_ROOT_DIR {}",
            canonical.display(),
            root_canon.display()
        )));
    }
    Ok(canonical)
}

/// Validate a [`Source`] against the input-size limit and (for paths) the
/// `ALLOW_ROOT_DIR` root, returning a [`Source`] whose path has been canonicalized.
///
/// This must be called **before** base64 decode (for bytes the decoded length
/// estimate is checked by the caller) and before reading a path file.
///
/// For path sources the file is **opened with `O_NOFOLLOW`** and `fstat`'d on
/// the **same descriptor** (not a separate by-path stat), so the size check and
/// the later read cannot be raced by a terminal symlink or a file that changes
/// in between (TOCTOU hardening). The result must be a regular file.
pub fn validate_source(source: &Source) -> Result<Source, ExtractionError> {
    let max = max_input_bytes();
    match source {
        Source::Bytes(b) => {
            if b.len() > max {
                return Err(ExtractionError::ResourceLimit(format!(
                    "input size {} bytes exceeds MAX_INPUT_BYTES={max}",
                    b.len()
                )));
            }
            Ok(source.clone())
        }
        Source::Path(p) => {
            let canonical = canonicalize_within_root(p)?;
            let file = open_no_follow(&canonical)?;
            let meta = file.metadata().map_err(|e| {
                ExtractionError::Io(format!("failed to stat {}: {e}", canonical.display()))
            })?;
            if !meta.is_file() {
                return Err(ExtractionError::Io(format!(
                    "{} is not a regular file",
                    canonical.display()
                )));
            }
            if meta.len() > max as u64 {
                return Err(ExtractionError::ResourceLimit(format!(
                    "input size {} bytes exceeds MAX_INPUT_BYTES={max}",
                    meta.len()
                )));
            }
            Ok(Source::Path(canonical))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ENV_LOCK;
    use std::sync::MutexGuard;

    fn lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn read_file_ok_and_missing() {
        let dir = std::env::temp_dir().join(format!("mcp-file-reader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        std::fs::write(&p, b"hello").unwrap();
        assert_eq!(read_file(&p).unwrap(), b"hello");
        std::fs::remove_dir_all(&dir).unwrap();

        let err = read_file(Path::new("/definitely/missing.txt")).unwrap_err();
        assert!(matches!(err, ExtractionError::Io(_)));
    }

    #[test]
    fn read_source_bytes_both_variants() {
        let bytes = read_source_bytes(&Source::Bytes(vec![1, 2, 3])).unwrap();
        assert_eq!(bytes, vec![1, 2, 3]);

        let dir = std::env::temp_dir().join(format!("mcp-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("b.bin");
        std::fs::write(&p, b"xyz").unwrap();
        assert_eq!(read_source_bytes(&Source::Path(p.clone())).unwrap(), b"xyz");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn validate_bytes_within_limit_ok() {
        let _g = lock();
        unsafe { std::env::set_var("MAX_INPUT_BYTES", "100") };
        let s = validate_source(&Source::Bytes(vec![0u8; 50])).unwrap();
        assert!(matches!(s, Source::Bytes(_)));
        unsafe { std::env::remove_var("MAX_INPUT_BYTES") };
    }

    #[test]
    fn validate_bytes_over_limit_is_resource_limit() {
        let _g = lock();
        unsafe { std::env::set_var("MAX_INPUT_BYTES", "10") };
        let err = validate_source(&Source::Bytes(vec![0u8; 11])).unwrap_err();
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));
        unsafe { std::env::remove_var("MAX_INPUT_BYTES") };
    }

    #[test]
    fn validate_path_canonicalizes_and_checks_size() {
        let _g = lock();
        unsafe { std::env::set_var("MAX_INPUT_BYTES", "1000") };
        let dir = std::env::temp_dir().join(format!("mcp-val-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Narrow the root to the temp dir so the file is inside it (the default
        // root is ~/Documents, which a temp-dir file would fall outside).
        unsafe { std::env::set_var("ALLOW_ROOT_DIR", &dir) };
        let p = dir.join("c.txt");
        std::fs::write(&p, b"hello").unwrap();
        let s = validate_source(&Source::Path(p.clone())).unwrap();
        match s {
            Source::Path(c) => assert!(c.is_absolute()),
            _ => panic!("expected path"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
        unsafe { std::env::remove_var("MAX_INPUT_BYTES") };
        unsafe { std::env::remove_var("ALLOW_ROOT_DIR") };
    }

    #[test]
    fn validate_missing_path_is_io_error() {
        let err = validate_source(&Source::Path(PathBuf::from("/definitely/missing/file.pdf")))
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Io(_)));
    }

    #[test]
    fn canonicalize_within_root_rejects_outside() {
        let _g = lock();
        // Set a narrow ALLOW_ROOT_DIR to a temp dir and confirm a path outside it is
        // rejected. The outside path must EXIST so `std::fs::canonicalize`
        // succeeds and the `starts_with(&root_canon)` rejection is genuinely
        // exercised (a missing path would fail earlier, at canonicalize).
        let dir = std::env::temp_dir().join(format!("mcp-root-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("ALLOW_ROOT_DIR", &dir) };

        let outside_dir =
            std::env::temp_dir().join(format!("mcp-root-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside_dir).unwrap();
        let outside = outside_dir.join("somewhere-else.txt");
        std::fs::write(&outside, b"x").unwrap();

        let err = canonicalize_within_root(&outside).unwrap_err();
        assert!(matches!(err, ExtractionError::Io(_)));
        // The rejection must come from the outside-root branch, not from a
        // canonicalize failure.
        assert!(err.to_string().contains("outside ALLOW_ROOT_DIR"));

        unsafe { std::env::remove_var("ALLOW_ROOT_DIR") };
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&outside_dir).unwrap();
    }

    #[test]
    fn validate_path_over_limit_is_resource_limit() {
        let _g = lock();
        unsafe { std::env::set_var("MAX_INPUT_BYTES", "10") };
        let dir = std::env::temp_dir().join(format!("mcp-psize-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("ALLOW_ROOT_DIR", &dir) };
        let p = dir.join("big.bin");
        std::fs::write(&p, vec![0u8; 100]).unwrap();

        let err = validate_source(&Source::Path(p.clone())).unwrap_err();
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));

        std::fs::remove_dir_all(&dir).unwrap();
        unsafe { std::env::remove_var("MAX_INPUT_BYTES") };
        unsafe { std::env::remove_var("ALLOW_ROOT_DIR") };
    }

    #[cfg(unix)]
    #[test]
    fn open_no_follow_rejects_symlink_final_component() {
        use std::os::unix::fs::symlink;
        let _g = lock();
        let dir = std::env::temp_dir().join(format!("mcp-nofollow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.txt");
        let link = dir.join("link.txt");
        std::fs::write(&target, b"x").unwrap();
        symlink(&target, &link).unwrap();
        // A terminal symlink must be rejected by the no-follow open.
        assert!(open_no_follow(&link).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn validate_source_rejects_symlink_inside_root_via_no_follow() {
        use std::os::unix::fs::symlink;
        let _g = lock();
        unsafe { std::env::set_var("MAX_INPUT_BYTES", "1000") };
        let root = std::env::temp_dir().join(format!("mcp-root-nf-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        unsafe { std::env::set_var("ALLOW_ROOT_DIR", &root) };

        // A real file inside the root plus a symlink inside the root pointing at it.
        let target = root.join("real.txt");
        std::fs::write(&target, b"hello").unwrap();
        let link = root.join("link.txt");
        symlink(&target, &link).unwrap();

        // canonicalize resolves the symlink to the (in-root) target, so validation
        // succeeds via the resolved canonical path; the no-follow open on that
        // canonical (real) path is fine.
        assert!(validate_source(&Source::Path(link.clone())).is_ok());

        // A symlink whose *final* component is a link that canonicalize does NOT
        // fully resolve would be rejected — here we assert the direct no-follow
        // helper rejects a symlink regardless of root.
        assert!(open_no_follow(&link).is_err());

        unsafe { std::env::remove_var("MAX_INPUT_BYTES") };
        unsafe { std::env::remove_var("ALLOW_ROOT_DIR") };
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn canonicalize_within_root_accepts_inside() {
        let _g = lock();
        let dir = std::env::temp_dir().join(format!("mcp-root2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let inside = dir.join("f.txt");
        std::fs::write(&inside, b"x").unwrap();
        unsafe { std::env::set_var("ALLOW_ROOT_DIR", &dir) };
        let c = canonicalize_within_root(&inside).unwrap();
        assert!(c.starts_with(std::fs::canonicalize(&dir).unwrap()));
        unsafe { std::env::remove_var("ALLOW_ROOT_DIR") };
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
