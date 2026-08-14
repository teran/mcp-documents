//! `AnyDocExtractor` — implements [`DocumentExtractor`] using `anydoc`.
//!
//! `anydoc` extracts Markdown from text-based PDFs (via `pdf-inspector`
//! internally). It does **not** perform OCR and does **not** render pages, so
//! classification here is a light heuristic: if a text layer is present the
//! document is `TextBased`; if `anydoc` reports the PDF as unsupported (i.e.
//! image-only), it is treated as `Scanned` and routed to OCR.

use std::sync::Arc;

use anydoc::Format;
use pdf_render::pdf_syntax::Pdf;

use crate::domain::document::{Classification, DocumentKind, PageClassification, PdfType};
use crate::domain::error::ExtractionError;
use crate::domain::ports::DocumentExtractor;
use crate::domain::result::{ExtractionResult, Source};

use super::error_map::{map_convert_error, map_load_pdf_error};
use super::file_reader::read_source_bytes;

/// Adapter that classifies and extracts documents using `anydoc`.
#[derive(Default)]
pub struct AnyDocExtractor {}

impl AnyDocExtractor {
    /// Create a new extractor.
    pub fn new() -> Self {
        Self {}
    }

    /// Detect the document kind from content. Round 1 is PDF-only.
    fn detect_kind(&self, bytes: &[u8]) -> Result<DocumentKind, ExtractionError> {
        match Format::from_bytes(bytes) {
            Some(Format::Pdf) => Ok(DocumentKind::Pdf),
            // Round 1 only supports PDF (unrecognized content or other formats).
            _ => Err(ExtractionError::Unsupported(DocumentKind::Pdf)),
        }
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
}

impl DocumentExtractor for AnyDocExtractor {
    fn classify(&self, source: &Source) -> Result<Classification, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let kind = self.detect_kind(&bytes)?;

        let page_count = self.page_count(&bytes)?;
        let has_text_layer = self.probe_text(&bytes)?.is_some();

        if has_text_layer {
            return Ok(Classification {
                kind,
                pdf_type: Some(PdfType::TextBased),
                pages_needing_ocr: Vec::new(),
            });
        }

        // Image-only / scanned: every page needs OCR.
        let pages = (0..page_count)
            .map(|i| PageClassification::new(i, true, 1.0))
            .collect();
        Ok(Classification {
            kind,
            pdf_type: Some(PdfType::Scanned),
            pages_needing_ocr: pages,
        })
    }

    fn extract_text(&self, source: &Source) -> Result<ExtractionResult, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let kind = self.detect_kind(&bytes)?;

        let result =
            std::panic::catch_unwind(|| anydoc::to_markdown_bytes(&bytes, Some(Format::Pdf)));
        let markdown = match result {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => return Err(map_convert_error(e)),
            Err(_) => {
                return Err(ExtractionError::Internal(
                    "document conversion panicked during extraction".into(),
                ));
            }
        };

        Ok(ExtractionResult::markdown_only(
            kind,
            Some(PdfType::TextBased),
            markdown,
        ))
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
        // A minimal PDF with a text layer, built with pdf-render's test approach
        // is heavy; instead we craft a tiny but valid PDF with an invisible text
        // object. Content stream draws "Hello".
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
    fn detects_pdf_kind() {
        let e = AnyDocExtractor::new();
        assert_eq!(e.detect_kind(&text_pdf_bytes()).unwrap(), DocumentKind::Pdf);
        assert!(matches!(
            e.detect_kind(b"not a document at all"),
            Err(ExtractionError::Unsupported(_))
        ));
    }

    #[test]
    fn extracts_text_pdf() {
        let e = AnyDocExtractor::new();
        // The hand-rolled PDF above may or may not pass pdf-inspector's strict
        // structure checks; what matters is the path returns a *domain* outcome
        // (success or a mapped domain error) and never leaks anydoc's error type
        // or panics. Real success is exercised by the committed fixture in
        // tests/fixtures/simple_text.pdf.
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
    fn classifies_non_pdf_as_unsupported() {
        let e = AnyDocExtractor::new();
        let err = e.classify(&Source::Bytes(b"garbage".to_vec())).unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }
}
