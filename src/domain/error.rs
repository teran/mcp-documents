//! Domain-native errors.

use crate::domain::document::DocumentKind;

/// Errors produced by the document extraction pipeline.
///
/// These are **domain-native** errors. Infrastructure adapters map third-party
/// library errors (e.g. `anydoc::ConvertError`) onto these variants; the
/// application and interface layers never leak library-specific error types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractionError {
    /// The format is not supported / cannot be converted.
    Unsupported(DocumentKind),
    /// The document is structurally unusable.
    Malformed(String),
    /// The document is encrypted or password-protected.
    Encrypted,
    /// A fixed safety / resource limit was exceeded (size, pages, ...).
    ResourceLimit(String),
    /// A part required for meaningful output is missing.
    MissingPart(String),
    /// An underlying I/O failure.
    Io(String),
    /// A scanned/image page needs OCR but no OCR provider is configured.
    OcrUnavailable(String),
    /// An unexpected / invariant violation.
    Internal(String),
}

impl ExtractionError {
    /// A stable, machine-readable name for the variant.
    pub fn code(&self) -> &'static str {
        match self {
            ExtractionError::Unsupported(_) => "unsupported",
            ExtractionError::Malformed(_) => "malformed",
            ExtractionError::Encrypted => "encrypted",
            ExtractionError::ResourceLimit(_) => "resourceLimit",
            ExtractionError::MissingPart(_) => "missingPart",
            ExtractionError::Io(_) => "io",
            ExtractionError::OcrUnavailable(_) => "ocrUnavailable",
            ExtractionError::Internal(_) => "internal",
        }
    }
}

impl std::fmt::Display for ExtractionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractionError::Unsupported(kind) => {
                write!(f, "unsupported document kind: {}", kind.as_str())
            }
            ExtractionError::Malformed(detail) => write!(f, "malformed document: {detail}"),
            ExtractionError::Encrypted => write!(f, "document is encrypted"),
            ExtractionError::ResourceLimit(detail) => {
                write!(f, "resource limit exceeded: {detail}")
            }
            ExtractionError::MissingPart(part) => write!(f, "missing required part: {part}"),
            ExtractionError::Io(e) => write!(f, "io error: {e}"),
            ExtractionError::OcrUnavailable(msg) => write!(f, "OCR unavailable: {msg}"),
            ExtractionError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for ExtractionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            ExtractionError::Unsupported(DocumentKind::Pdf).code(),
            "unsupported"
        );
        assert_eq!(ExtractionError::Malformed("x".into()).code(), "malformed");
        assert_eq!(ExtractionError::Encrypted.code(), "encrypted");
        assert_eq!(
            ExtractionError::ResourceLimit("x".into()).code(),
            "resourceLimit"
        );
        assert_eq!(
            ExtractionError::MissingPart("x".into()).code(),
            "missingPart"
        );
        assert_eq!(ExtractionError::Io("x".into()).code(), "io");
        assert_eq!(
            ExtractionError::OcrUnavailable("x".into()).code(),
            "ocrUnavailable"
        );
        assert_eq!(ExtractionError::Internal("x".into()).code(), "internal");
    }

    #[test]
    fn display_contains_message() {
        let e = ExtractionError::Malformed("broken".into());
        assert!(e.to_string().contains("broken"));
        assert!(ExtractionError::Encrypted.to_string().contains("encrypted"));
    }

    #[test]
    fn implements_std_error() {
        fn assert_error<T: std::error::Error>() {}
        assert_error::<ExtractionError>();
    }
}
