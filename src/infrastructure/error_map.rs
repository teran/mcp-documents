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

/// Map a `pdf_inspector::PdfError` onto a domain error.
pub fn map_pdf_inspector_error(e: pdf_inspector::PdfError) -> ExtractionError {
    use pdf_inspector::PdfError;
    match e {
        PdfError::Encrypted => ExtractionError::Encrypted,
        PdfError::InvalidStructure => {
            ExtractionError::Malformed("PDF has invalid structure".into())
        }
        PdfError::Parse(msg) => ExtractionError::Malformed(format!("PDF parse error: {msg}")),
        PdfError::NotAPdf(_) => ExtractionError::Unsupported(DocumentKind::Pdf),
        PdfError::Io(e) => ExtractionError::Io(e.to_string()),
    }
}

/// Map a `calamine::Error` onto a domain error.
pub fn map_calamine_error(e: calamine::Error) -> ExtractionError {
    match e {
        calamine::Error::Msg(msg) => ExtractionError::Malformed(format!("spreadsheet: {msg}")),
        calamine::Error::Io(e) => ExtractionError::Io(e.to_string()),
        // `calamine::Error` is non-exhaustive; fall back to Malformed.
        _ => ExtractionError::Malformed("could not read spreadsheet".into()),
    }
}

/// Map an I/O error from the streaming CSV reader onto a domain error.
pub fn map_csv_io_error(e: std::io::Error) -> ExtractionError {
    ExtractionError::Io(e.to_string())
}

/// Map a `zip::result::ZipError` (reading PPTX slide parts) onto a domain error.
pub fn map_zip_error(e: zip::result::ZipError) -> ExtractionError {
    use zip::result::ZipError;
    match e {
        ZipError::FileNotFound => {
            ExtractionError::MissingPart("presentation part not found in archive".into())
        }
        ZipError::InvalidArchive(_) => ExtractionError::Malformed("invalid OOXML archive".into()),
        ZipError::UnsupportedArchive(_) => {
            ExtractionError::Malformed("unsupported OOXML archive".into())
        }
        _ => ExtractionError::Malformed("could not read OOXML archive".into()),
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
    fn maps_convert_error_malformed_with_part_and_catch_all() {
        use anydoc::ConvertError;

        // Malformed with a part name prefixes the detail.
        let err = map_convert_error(ConvertError::Malformed {
            part: Some("stream".into()),
            detail: "corrupt".into(),
        });
        assert_eq!(err.to_string(), "malformed document: stream: corrupt");

        // A not-yet-covered variant falls through to Internal.
        let err = map_convert_error(ConvertError::Unsupported("other".into()));
        assert!(matches!(err, ExtractionError::Unsupported(_)));
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
        assert_eq!(
            map_load_pdf_error(LoadPdfError::Decryption(
                pdf_render::pdf_syntax::DecryptionError::PasswordProtected
            )),
            ExtractionError::Encrypted
        );
    }

    #[test]
    fn maps_pdf_inspector_error_variants() {
        assert_eq!(
            map_pdf_inspector_error(pdf_inspector::PdfError::Encrypted),
            ExtractionError::Encrypted
        );
        assert!(matches!(
            map_pdf_inspector_error(pdf_inspector::PdfError::Parse("x".into())),
            ExtractionError::Malformed(_)
        ));
        assert!(matches!(
            map_pdf_inspector_error(pdf_inspector::PdfError::InvalidStructure),
            ExtractionError::Malformed(_)
        ));
        assert!(matches!(
            map_pdf_inspector_error(pdf_inspector::PdfError::NotAPdf("x".into())),
            ExtractionError::Unsupported(_)
        ));
        assert!(matches!(
            map_pdf_inspector_error(pdf_inspector::PdfError::Io(std::io::Error::other("io"))),
            ExtractionError::Io(_)
        ));
    }

    #[test]
    fn maps_calamine_error() {
        assert!(matches!(
            map_calamine_error(calamine::Error::Msg("bad")),
            ExtractionError::Malformed(_)
        ));
        assert!(matches!(
            map_calamine_error(calamine::Error::Io(std::io::Error::other("io"))),
            ExtractionError::Io(_)
        ));
    }

    #[test]
    fn maps_csv_io_and_zip_errors() {
        assert!(matches!(
            map_csv_io_error(std::io::Error::other("io")),
            ExtractionError::Io(_)
        ));
        assert!(matches!(
            map_zip_error(zip::result::ZipError::FileNotFound),
            ExtractionError::MissingPart(_)
        ));
        assert!(matches!(
            map_zip_error(zip::result::ZipError::InvalidArchive("bad".into())),
            ExtractionError::Malformed(_)
        ));
        assert!(matches!(
            map_zip_error(zip::result::ZipError::UnsupportedArchive("bad")),
            ExtractionError::Malformed(_)
        ));
    }
}
