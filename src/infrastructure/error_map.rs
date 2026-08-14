//! Mapping of third-party library errors onto domain [`ExtractionError`]s.
//!
//! This guarantees the application/interface layers never leak library-specific
//! error types (e.g. `anydoc::ConvertError`, `pdf-syntax::LoadPdfError`).

use crate::domain::document::DocumentKind;
use crate::domain::error::ExtractionError;

/// Map an `anydoc::ConvertError` (or any source of one) onto a domain error.
pub fn map_convert_error(e: anydoc::ConvertError) -> ExtractionError {
    use anydoc::ConvertError;
    match e {
        ConvertError::Unsupported(_) => ExtractionError::Unsupported(DocumentKind::Pdf),
        ConvertError::Malformed { part, detail } => {
            let detail = match part {
                Some(p) => format!("{p}: {detail}"),
                None => detail,
            };
            ExtractionError::Malformed(detail)
        }
        ConvertError::Encrypted => ExtractionError::Encrypted,
        ConvertError::ResourceLimit { limit, detail } => {
            ExtractionError::ResourceLimit(format!("{limit}: {detail}"))
        }
        ConvertError::MissingPart { part } => ExtractionError::MissingPart(part),
        ConvertError::Io(e) => ExtractionError::Io(e.to_string()),
        // `ConvertError` is `#[non_exhaustive]`.
        _ => ExtractionError::Internal("unexpected conversion error".into()),
    }
}

/// Map a `pdf-syntax::LoadPdfError` (from rendering/parsing a PDF) onto a
/// domain error.
pub fn map_load_pdf_error(e: pdf_render::pdf_syntax::LoadPdfError) -> ExtractionError {
    use pdf_render::pdf_syntax::LoadPdfError;
    match e {
        LoadPdfError::Decryption(_) => ExtractionError::Encrypted,
        LoadPdfError::Invalid => ExtractionError::Malformed("PDF could not be parsed".into()),
        LoadPdfError::TooLarge(objects, pages) => ExtractionError::ResourceLimit(format!(
            "PDF too large: {objects} objects, {pages} pages"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_convert_error_variants() {
        use anydoc::ConvertError;

        let err = map_convert_error(ConvertError::Unsupported("pdf".into()));
        assert!(matches!(
            err,
            ExtractionError::Unsupported(DocumentKind::Pdf)
        ));

        let err = map_convert_error(ConvertError::Malformed {
            part: None,
            detail: "bad".into(),
        });
        assert!(matches!(err, ExtractionError::Malformed(_)));

        let err = map_convert_error(ConvertError::Encrypted);
        assert_eq!(err, ExtractionError::Encrypted);

        let err = map_convert_error(ConvertError::ResourceLimit {
            limit: "max_pages",
            detail: "100".into(),
        });
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));

        let err = map_convert_error(ConvertError::MissingPart { part: "x".into() });
        assert!(matches!(err, ExtractionError::MissingPart(_)));

        let err = map_convert_error(ConvertError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "gone",
        )));
        assert!(matches!(err, ExtractionError::Io(_)));
    }

    #[test]
    fn maps_load_pdf_error_variants() {
        use pdf_render::pdf_syntax::LoadPdfError;

        assert_eq!(
            map_load_pdf_error(LoadPdfError::Invalid),
            ExtractionError::Malformed("PDF could not be parsed".into())
        );
        assert_eq!(
            map_load_pdf_error(LoadPdfError::TooLarge(5, 6)),
            ExtractionError::ResourceLimit("PDF too large: 5 objects, 6 pages".into())
        );
    }
}
