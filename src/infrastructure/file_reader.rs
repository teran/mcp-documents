//! File reading helpers (path → bytes).

use crate::domain::error::ExtractionError;
use crate::domain::result::Source;

/// Read a file from disk into bytes.
pub fn read_file(path: &std::path::Path) -> Result<Vec<u8>, ExtractionError> {
    std::fs::read(path)
        .map_err(|e| ExtractionError::Io(format!("failed to read {}: {e}", path.display())))
}

/// Resolve a [`Source`] to bytes, reading from disk when it is a path.
pub fn read_source_bytes(source: &Source) -> Result<Vec<u8>, ExtractionError> {
    source.read_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
}
