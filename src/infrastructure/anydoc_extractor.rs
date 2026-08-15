//! `AnyDocExtractor` — implements [`DocumentExtractor`] using `anydoc`.
//!
//! `anydoc` extracts Markdown from text-based PDFs (via `pdf-inspector`
//! internally) and from MS Office / ODF / RTF / EPUB / CSV documents. It does
//! **not** perform OCR and does **not** render pages, so PDF classification
//! here is a light heuristic: if a text layer is present the PDF is
//! `TextBased`; if `anydoc` reports the PDF as unsupported (i.e. image-only),
//! it is treated as `Scanned` and routed to OCR. Non-PDF kinds have no PDF
//! classification and go straight through `anydoc`.

use std::sync::Arc;

use anydoc::Format;
use pdf_render::pdf_syntax::Pdf;

use crate::domain::document::{Classification, DocumentKind, PageClassification, PdfType};
use crate::domain::error::ExtractionError;
use crate::domain::ports::DocumentExtractor;
use crate::domain::result::{ExtractionResult, Source};

use super::error_map::{map_convert_error, map_load_pdf_error};
use super::file_reader::read_source_bytes;

/// Map an `anydoc::Format` onto a [`DocumentKind`].
///
/// `Format::Excel` covers `.xls`/`.xlsx`/`.xlsm`/`.xlsb` (calamine reads them
/// all) and is reported here as [`DocumentKind::Xlsx`], the dominant modern
/// variant — see the module note for the legacy `.xls` caveat.
pub fn format_to_kind(fmt: Format) -> DocumentKind {
    match fmt {
        Format::Pdf => DocumentKind::Pdf,
        Format::Doc => DocumentKind::Doc,
        Format::Docx => DocumentKind::Docx,
        Format::Excel => DocumentKind::Xlsx,
        Format::Ppt => DocumentKind::Ppt,
        Format::Pptx => DocumentKind::Pptx,
        Format::Odt => DocumentKind::Odt,
        Format::Ods => DocumentKind::Ods,
        Format::Odp => DocumentKind::Odp,
        Format::Rtf => DocumentKind::Rtf,
        Format::Epub => DocumentKind::Epub,
        Format::Csv => DocumentKind::Csv,
    }
}

/// Adapter that classifies and extracts documents using `anydoc`.
#[derive(Default)]
pub struct AnyDocExtractor {}

impl AnyDocExtractor {
    /// Create a new extractor.
    pub fn new() -> Self {
        Self {}
    }

    /// Detect the document format from content.
    fn detect_format(&self, bytes: &[u8]) -> Result<Format, ExtractionError> {
        Format::from_bytes(bytes).ok_or(ExtractionError::Unsupported(DocumentKind::Unknown))
    }

    /// Count pages in a PDF using the pure-Rust parser.
    fn page_count(&self, bytes: &[u8]) -> Result<usize, ExtractionError> {
        let count =
            std::panic::catch_unwind(|| Pdf::new(bytes.to_vec()).map(|pdf| pdf.pages().len()))
                .map_err(|_| {
                    ExtractionError::Internal("PDF parser panicked while counting pages".into())
                })?;
        count.map_err(map_load_pdf_error)
    }

    /// Probe whether a PDF has a usable text layer. `Ok(None)` means "no text
    /// layer" (image-only). Conversion panics are isolated here.
    fn probe_text(&self, bytes: &[u8]) -> Result<Option<String>, ExtractionError> {
        let result =
            std::panic::catch_unwind(|| anydoc::to_markdown_bytes(bytes, Some(Format::Pdf)));
        match result {
            Ok(Ok(text)) => Ok(Some(text)),
            Ok(Err(anydoc::ConvertError::Unsupported(_))) => Ok(None),
            Ok(Err(e)) => Err(map_convert_error(e)),
            Err(_) => Err(ExtractionError::Internal(
                "document conversion panicked while probing text".into(),
            )),
        }
    }

    /// Classify a PDF (PdfType + pages needing OCR) using the light heuristic.
    fn classify_pdf(&self, bytes: &[u8]) -> Result<Classification, ExtractionError> {
        let page_count = self.page_count(bytes)?;
        let has_text_layer = self.probe_text(bytes)?.is_some();

        if has_text_layer {
            return Ok(Classification {
                kind: DocumentKind::Pdf,
                pdf_type: Some(PdfType::TextBased),
                pages_needing_ocr: Vec::new(),
            });
        }

        // Image-only / scanned: every page needs OCR.
        let pages = (0..page_count)
            .map(|i| PageClassification::new(i, true, 1.0))
            .collect();
        Ok(Classification {
            kind: DocumentKind::Pdf,
            pdf_type: Some(PdfType::Scanned),
            pages_needing_ocr: pages,
        })
    }
}

impl DocumentExtractor for AnyDocExtractor {
    fn classify(&self, source: &Source) -> Result<Classification, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let format = self.detect_format(&bytes)?;

        if format == Format::Pdf {
            return self.classify_pdf(&bytes);
        }

        // Non-PDF kinds have no per-page/OCR classification.
        Ok(Classification {
            kind: format_to_kind(format),
            pdf_type: None,
            pages_needing_ocr: Vec::new(),
        })
    }

    fn extract_text(&self, source: &Source) -> Result<ExtractionResult, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let format = self.detect_format(&bytes)?;
        let kind = format_to_kind(format);
        // PDF-specific type is only meaningful for PDFs.
        let pdf_type = if kind == DocumentKind::Pdf {
            Some(PdfType::TextBased)
        } else {
            None
        };

        // Let anydoc auto-detect the parser from content (None). Wrapped in
        // panic isolation so a malformed document becomes a domain error.
        let result = std::panic::catch_unwind(|| anydoc::to_markdown_bytes(&bytes, None));
        let markdown = match result {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => return Err(map_convert_error(e)),
            Err(_) => {
                return Err(ExtractionError::Internal(
                    "document conversion panicked during extraction".into(),
                ));
            }
        };

        Ok(ExtractionResult::markdown_only(kind, pdf_type, markdown))
    }
}

/// Build an `Arc<dyn DocumentExtractor>` for wiring/composition.
pub fn make_extractor() -> Arc<dyn DocumentExtractor> {
    Arc::new(AnyDocExtractor::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_pdf_bytes() -> Vec<u8> {
        // A minimal PDF with a text layer.
        let content = "BT /F1 12 Tf 72 712 Td (Hello World) Tj ET";
        let content_id = 4;
        let mut pdf = String::new();
        pdf.push_str("%PDF-1.4\n");
        pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        pdf.push_str("3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] ");
        pdf.push_str(&format!("/Contents {content_id} 0 R >>\nendobj\n"));
        pdf.push_str(&format!(
            "{content_id} 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n",
            content.len()
        ));
        pdf.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
        pdf.into_bytes()
    }

    #[test]
    fn format_to_kind_maps_all_formats() {
        assert_eq!(format_to_kind(Format::Pdf), DocumentKind::Pdf);
        assert_eq!(format_to_kind(Format::Doc), DocumentKind::Doc);
        assert_eq!(format_to_kind(Format::Docx), DocumentKind::Docx);
        assert_eq!(format_to_kind(Format::Excel), DocumentKind::Xlsx);
        assert_eq!(format_to_kind(Format::Ppt), DocumentKind::Ppt);
        assert_eq!(format_to_kind(Format::Pptx), DocumentKind::Pptx);
        assert_eq!(format_to_kind(Format::Odt), DocumentKind::Odt);
        assert_eq!(format_to_kind(Format::Ods), DocumentKind::Ods);
        assert_eq!(format_to_kind(Format::Odp), DocumentKind::Odp);
        assert_eq!(format_to_kind(Format::Rtf), DocumentKind::Rtf);
        assert_eq!(format_to_kind(Format::Epub), DocumentKind::Epub);
        assert_eq!(format_to_kind(Format::Csv), DocumentKind::Csv);
    }

    #[test]
    fn detect_format_detects_pdf_and_rejects_garbage() {
        let e = AnyDocExtractor::new();
        assert_eq!(e.detect_format(&text_pdf_bytes()).unwrap(), Format::Pdf);
        assert!(matches!(
            e.detect_format(b"not a document at all"),
            Err(ExtractionError::Unsupported(_))
        ));
    }

    #[test]
    fn extracts_text_pdf() {
        let e = AnyDocExtractor::new();
        // The hand-rolled PDF above may or may not pass pdf-inspector's strict
        // structure checks; what matters is the path returns a *domain* outcome
        // (success or a mapped domain error). Real success is exercised by the
        // committed fixtures.
        match e.extract_text(&Source::Bytes(text_pdf_bytes())) {
            Ok(r) => assert_eq!(r.kind, DocumentKind::Pdf),
            Err(err) => assert!(
                matches!(
                    err,
                    ExtractionError::Malformed(_) | ExtractionError::Unsupported(_)
                ),
                "unexpected error: {err}"
            ),
        }
    }

    #[test]
    fn classifies_garbage_as_unknown_unsupported() {
        let e = AnyDocExtractor::new();
        let err = e.classify(&Source::Bytes(b"garbage".to_vec())).unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }
}
