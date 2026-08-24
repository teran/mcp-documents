//! `AnyDocExtractor` — implements [`DocumentExtractor`] using `anydoc` for
//! format detection / whole-document Markdown, and `pdf-inspector` for real
//! per-page PDF classification.
//!
//! `anydoc` extracts Markdown from MS Office / ODF / RTF / EPUB / CSV and from
//! text-based PDFs (via `pdf-inspector` internally). It does **not** perform OCR
//! and does **not** render pages, so PDF classification here uses
//! `pdf-inspector`'s per-page detector directly to produce a real
//! `TextBased`/`Scanned`/`ImageBased`/`Mixed` result plus the exact pages that
//! need OCR.

use std::sync::Arc;

use anydoc::Format;
use pdf_inspector::{DetectionConfig, ScanStrategy};

use crate::domain::document::{Classification, DocumentKind, PageClassification, PdfType};
use crate::domain::error::ExtractionError;
use crate::domain::ports::DocumentExtractor;
use crate::domain::result::{ExtractionResult, Source};

use super::error_map::{map_convert_error, map_pdf_inspector_error};
use super::file_reader::read_source_bytes;

/// Map an `anydoc::Format` onto a [`DocumentKind`].
///
/// Excel covers `.xls`/`.xlsx`/`.xlsm`/`.xlsb`; the exact kind is resolved from
/// the file's magic bytes (see [`format_to_kind`]).
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

/// Distinguish `.xls` from `.xlsx`/`.xlsm` by magic bytes.
///
/// `.xlsx`/`.xlsm` are ZIP archives (`PK\x03\x04`); `.xls`/`.xlsb` are OLE
/// compound files (`D0 CF 11 E0`). When the format is ambiguous we fall back to
/// the dominant modern variant, `Xlsx`.
fn excel_kind(bytes: &[u8]) -> DocumentKind {
    if bytes.starts_with(b"PK\x03\x04") {
        DocumentKind::Xlsx
    } else if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        DocumentKind::Xls
    } else {
        DocumentKind::Xlsx
    }
}

/// Map a `pdf_inspector::PdfType` onto our domain [`PdfType`].
pub fn map_pdf_inspector_type(t: pdf_inspector::PdfType) -> PdfType {
    match t {
        pdf_inspector::PdfType::TextBased => PdfType::TextBased,
        pdf_inspector::PdfType::Scanned => PdfType::Scanned,
        pdf_inspector::PdfType::ImageBased => PdfType::ImageBased,
        pdf_inspector::PdfType::Mixed => PdfType::Mixed,
    }
}

/// Adapter that classifies and (for non-PDF) extracts documents.
#[derive(Default)]
pub struct AnyDocExtractor {}

impl AnyDocExtractor {
    /// Create a new extractor.
    pub fn new() -> Self {
        Self {}
    }

    /// Detect the document format from content, falling back to the file
    /// extension for path sources (e.g. CSV, which has no content signature).
    fn detect_format(&self, source: &Source) -> Result<Format, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let by_content = std::panic::catch_unwind(|| Format::from_bytes(&bytes))
            .map_err(|_| ExtractionError::Internal("format detection panicked".into()))?;
        if let Some(fmt) = by_content {
            return Ok(fmt);
        }
        if let Some(path) = source.path()
            && let Some(fmt) = Format::from_path(path)
        {
            return Ok(fmt);
        }
        Err(ExtractionError::Unsupported(DocumentKind::Unknown))
    }

    /// Real per-page PDF classification using `pdf-inspector`'s detector.
    fn classify_pdf(&self, bytes: &[u8]) -> Result<Classification, ExtractionError> {
        let config = DetectionConfig {
            // Scan every page so Mixed / Scanned / ImageBased are accurate.
            strategy: ScanStrategy::Full,
            ..Default::default()
        };
        let result = std::panic::catch_unwind(|| {
            pdf_inspector::detect_pdf_type_mem_with_config(bytes, config)
        });
        let result = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(map_pdf_inspector_error(e)),
            Err(_) => {
                return Err(ExtractionError::Internal(
                    "PDF detector panicked while classifying".into(),
                ));
            }
        };

        // pdf-inspector reports 1-indexed pages needing OCR.
        let pages = result
            .pages_needing_ocr
            .iter()
            .map(|&p| PageClassification::new(p as usize - 1, true, 1.0))
            .collect();
        Ok(Classification {
            kind: DocumentKind::Pdf,
            pdf_type: Some(map_pdf_inspector_type(result.pdf_type)),
            pages_needing_ocr: pages,
        })
    }
}

impl DocumentExtractor for AnyDocExtractor {
    fn classify(&self, source: &Source) -> Result<Classification, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let format = self.detect_format(source)?;

        if format == Format::Pdf {
            return self.classify_pdf(&bytes);
        }

        // Non-PDF kinds have no per-page/OCR classification.
        let kind = if format == Format::Excel {
            excel_kind(&bytes)
        } else {
            format_to_kind(format)
        };
        Ok(Classification {
            kind,
            pdf_type: None,
            pages_needing_ocr: Vec::new(),
        })
    }

    fn extract_text(&self, source: &Source) -> Result<ExtractionResult, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let format = self.detect_format(source)?;
        let kind = if format == Format::Excel {
            excel_kind(&bytes)
        } else {
            format_to_kind(format)
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

        Ok(ExtractionResult::markdown_only(kind, None, markdown))
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
    fn excel_kind_distinguishes_by_magic() {
        assert_eq!(excel_kind(b"PK\x03\x04 rest"), DocumentKind::Xlsx);
        assert_eq!(
            excel_kind(&[0xD0, 0xCF, 0x11, 0xE0, 1, 2]),
            DocumentKind::Xls
        );
        assert_eq!(excel_kind(b"garbage"), DocumentKind::Xlsx);
    }

    #[test]
    fn maps_pdf_inspector_types() {
        assert_eq!(
            map_pdf_inspector_type(pdf_inspector::PdfType::TextBased),
            PdfType::TextBased
        );
        assert_eq!(
            map_pdf_inspector_type(pdf_inspector::PdfType::Scanned),
            PdfType::Scanned
        );
        assert_eq!(
            map_pdf_inspector_type(pdf_inspector::PdfType::ImageBased),
            PdfType::ImageBased
        );
        assert_eq!(
            map_pdf_inspector_type(pdf_inspector::PdfType::Mixed),
            PdfType::Mixed
        );
    }

    #[test]
    fn detect_format_detects_pdf_and_rejects_garbage() {
        let e = AnyDocExtractor::new();
        assert_eq!(
            e.detect_format(&Source::Bytes(text_pdf_bytes())).unwrap(),
            Format::Pdf
        );
        assert!(matches!(
            e.detect_format(&Source::Bytes(b"not a document at all".to_vec())),
            Err(ExtractionError::Unsupported(_))
        ));
    }

    #[test]
    fn detect_format_uses_extension_fallback_for_csv() {
        let e = AnyDocExtractor::new();
        // CSV has no content signature; the extension fallback must detect it.
        let dir = std::env::temp_dir().join(format!("mcp-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("data.csv");
        std::fs::write(&p, "a,b\n1,2\n").unwrap();
        assert_eq!(
            e.detect_format(&Source::Path(p.clone())).unwrap(),
            Format::Csv
        );
        // classify via path resolves to Csv.
        let c = e.classify(&Source::Path(p)).unwrap();
        assert_eq!(c.kind, DocumentKind::Csv);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn classifies_garbage_as_unknown_unsupported() {
        let e = AnyDocExtractor::new();
        let err = e.classify(&Source::Bytes(b"garbage".to_vec())).unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }
}
