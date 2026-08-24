//! `SpreadsheetUseCase` — spreadsheets (xls / xlsx): sheet listing + sheet/row
//! extraction.

use std::ops::Range;
use std::sync::Arc;

use crate::config::max_return_chars;
use crate::domain::document::DocumentKind;
use crate::domain::error::ExtractionError;
use crate::domain::meta::SpreadsheetInfo;
use crate::domain::ports::{DocumentExtractor, SpreadsheetReader};
use crate::domain::result::{ExtractionResult, Source};

/// Orchestrates spreadsheet metadata and extraction.
#[derive(Clone)]
pub struct SpreadsheetUseCase {
    classifier: Arc<dyn DocumentExtractor>,
    reader: Arc<dyn SpreadsheetReader>,
    /// Safety-net cap on the size of the returned Markdown (`MAX_RETURN_CHARS`).
    max_return_chars: usize,
}

impl SpreadsheetUseCase {
    /// Build the use case around its injected ports, reading the char cap from
    /// env (`MAX_RETURN_CHARS`).
    pub fn new(classifier: Arc<dyn DocumentExtractor>, reader: Arc<dyn SpreadsheetReader>) -> Self {
        Self::new_with_max_chars(classifier, reader, max_return_chars())
    }

    /// Build with an explicit char cap (used by tests to avoid env races).
    pub fn new_with_max_chars(
        classifier: Arc<dyn DocumentExtractor>,
        reader: Arc<dyn SpreadsheetReader>,
        max_return_chars: usize,
    ) -> Self {
        Self {
            classifier,
            reader,
            max_return_chars,
        }
    }

    /// Verify the source is exactly `expected` (`Xls` or `Xlsx`), returning a
    /// clear tool-level error otherwise.
    fn require_kind(
        &self,
        source: &Source,
        expected: DocumentKind,
    ) -> Result<DocumentKind, ExtractionError> {
        let c = self.classifier.classify(source)?;
        if c.kind != expected {
            return Err(ExtractionError::Unsupported(c.kind));
        }
        Ok(c.kind)
    }

    /// List the workbook's sheets.
    pub fn info(
        &self,
        source: &Source,
        expected: DocumentKind,
    ) -> Result<SpreadsheetInfo, ExtractionError> {
        let _kind = self.require_kind(source, expected)?;
        self.reader.spreadsheet_info(source)
    }

    /// Extract a sheet (by name, or the first when `None`) as a Markdown table,
    /// restricted to the optional 0-based `row_range`. Enforces
    /// `MAX_RETURN_CHARS` on the returned table.
    pub fn extract_markdown(
        &self,
        source: &Source,
        expected: DocumentKind,
        sheet: Option<&str>,
        row_range: Option<Range<usize>>,
    ) -> Result<ExtractionResult, ExtractionError> {
        let kind = self.require_kind(source, expected)?;
        let markdown = self.reader.extract_sheet(source, sheet, row_range)?;
        let char_count = markdown.chars().count();
        if char_count > self.max_return_chars {
            return Err(ExtractionError::ResourceLimit(format!(
                "extracted {char_count} characters, exceeding MAX_RETURN_CHARS={}",
                self.max_return_chars
            )));
        }
        Ok(ExtractionResult::markdown_only(kind, None, markdown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::document::{Classification, PdfType};
    use crate::domain::meta::SheetInfo;

    #[derive(Clone)]
    struct FakeClassifier {
        kind: DocumentKind,
    }
    impl DocumentExtractor for FakeClassifier {
        fn classify(&self, _source: &Source) -> Result<Classification, ExtractionError> {
            let pdf_type = if self.kind == DocumentKind::Pdf {
                Some(PdfType::TextBased)
            } else {
                None
            };
            Ok(Classification {
                kind: self.kind,
                pdf_type,
                pages_needing_ocr: Vec::new(),
            })
        }
        fn extract_text(&self, _source: &Source) -> Result<ExtractionResult, ExtractionError> {
            unreachable!()
        }
    }

    #[derive(Clone)]
    struct FakeReader {
        info: SpreadsheetInfo,
        markdown: String,
    }
    impl SpreadsheetReader for FakeReader {
        fn spreadsheet_info(&self, _source: &Source) -> Result<SpreadsheetInfo, ExtractionError> {
            Ok(self.info.clone())
        }
        fn extract_sheet(
            &self,
            _source: &Source,
            _sheet: Option<&str>,
            _row_range: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok(self.markdown.clone())
        }
    }

    fn uc(kind: DocumentKind) -> SpreadsheetUseCase {
        let info = SpreadsheetInfo {
            sheets: vec![SheetInfo {
                name: "Sheet1".into(),
                rows: Some(2),
                cols: Some(2),
            }],
        };
        SpreadsheetUseCase::new(
            Arc::new(FakeClassifier { kind }),
            Arc::new(FakeReader {
                info,
                markdown: "| a | b |".into(),
            }),
        )
    }

    #[test]
    fn info_returns_sheets() {
        let u = uc(DocumentKind::Xlsx);
        let info = u.info(&Source::Bytes(vec![1]), DocumentKind::Xlsx).unwrap();
        assert_eq!(info.sheets[0].name, "Sheet1");
    }

    #[test]
    fn non_spreadsheet_is_rejected() {
        let u = uc(DocumentKind::Docx);
        let err = u
            .info(&Source::Bytes(vec![1]), DocumentKind::Xlsx)
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }

    #[test]
    fn extract_markdown_returns_sheet() {
        let u = uc(DocumentKind::Xls);
        let out = u
            .extract_markdown(
                &Source::Bytes(vec![1]),
                DocumentKind::Xls,
                Some("Sheet1"),
                None,
            )
            .unwrap();
        assert_eq!(out.markdown, "| a | b |");
        assert_eq!(out.kind, DocumentKind::Xls);
    }

    #[test]
    fn extract_markdown_char_cap_is_enforced() {
        let u = SpreadsheetUseCase::new_with_max_chars(
            Arc::new(FakeClassifier {
                kind: DocumentKind::Xlsx,
            }),
            Arc::new(FakeReader {
                info: SpreadsheetInfo { sheets: vec![] },
                markdown: "a very long table body".into(),
            }),
            5,
        );
        let err = u
            .extract_markdown(&Source::Bytes(vec![1]), DocumentKind::Xlsx, None, None)
            .unwrap_err();
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));
    }
}
