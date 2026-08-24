//! `CsvUseCase` — comma-separated values: streaming info + row-range extraction.

use std::ops::Range;
use std::sync::Arc;

use crate::config::max_return_chars;
use crate::domain::document::DocumentKind;
use crate::domain::error::ExtractionError;
use crate::domain::meta::CsvInfo;
use crate::domain::ports::{CsvReader, DocumentExtractor};
use crate::domain::result::{ExtractionResult, Source};

/// Orchestrates CSV metadata and extraction via the streaming CSV reader.
#[derive(Clone)]
pub struct CsvUseCase {
    classifier: Arc<dyn DocumentExtractor>,
    reader: Arc<dyn CsvReader>,
    /// Safety-net cap on the size of the returned Markdown (`MAX_RETURN_CHARS`).
    max_return_chars: usize,
}

impl CsvUseCase {
    /// Build the use case around its injected ports, reading the char cap from
    /// env (`MAX_RETURN_CHARS`).
    pub fn new(classifier: Arc<dyn DocumentExtractor>, reader: Arc<dyn CsvReader>) -> Self {
        Self::new_with_max_chars(classifier, reader, max_return_chars())
    }

    /// Build with an explicit char cap (used by tests to avoid env races).
    pub fn new_with_max_chars(
        classifier: Arc<dyn DocumentExtractor>,
        reader: Arc<dyn CsvReader>,
        max_return_chars: usize,
    ) -> Self {
        Self {
            classifier,
            reader,
            max_return_chars,
        }
    }

    /// Verify the source is actually a CSV, returning a clear tool-level error
    /// otherwise.
    fn require_csv(&self, source: &Source) -> Result<(), ExtractionError> {
        let c = self.classifier.classify(source)?;
        if c.kind != DocumentKind::Csv {
            return Err(ExtractionError::Unsupported(c.kind));
        }
        Ok(())
    }

    /// One streaming pass: total rows, column count and header.
    pub fn info(&self, source: &Source) -> Result<CsvInfo, ExtractionError> {
        self.require_csv(source)?;
        self.reader.csv_info(source)
    }

    /// Extract the requested 0-based `row_range` (default all rows) as a
    /// Markdown table. Skips to the start row and reads only the range.
    /// Enforces `MAX_RETURN_CHARS` on the returned table.
    pub fn extract_markdown(
        &self,
        source: &Source,
        row_range: Option<Range<usize>>,
    ) -> Result<ExtractionResult, ExtractionError> {
        self.require_csv(source)?;
        let markdown = self.reader.extract_csv(source, row_range)?;
        let char_count = markdown.chars().count();
        if char_count > self.max_return_chars {
            return Err(ExtractionError::ResourceLimit(format!(
                "extracted {char_count} characters, exceeding MAX_RETURN_CHARS={}",
                self.max_return_chars
            )));
        }
        Ok(ExtractionResult::markdown_only(
            DocumentKind::Csv,
            None,
            markdown,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::document::{Classification, PdfType};

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
        info: CsvInfo,
        markdown: String,
    }
    impl CsvReader for FakeReader {
        fn csv_info(&self, _source: &Source) -> Result<CsvInfo, ExtractionError> {
            Ok(self.info.clone())
        }
        fn extract_csv(
            &self,
            _source: &Source,
            _row_range: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok(self.markdown.clone())
        }
    }

    fn uc(kind: DocumentKind) -> CsvUseCase {
        let info = CsvInfo {
            row_count: 3,
            column_count: 2,
            header: vec!["a".into(), "b".into()],
        };
        CsvUseCase::new(
            Arc::new(FakeClassifier { kind }),
            Arc::new(FakeReader {
                info,
                markdown: "| a | b |".into(),
            }),
        )
    }

    #[test]
    fn info_returns_csv_metadata() {
        let u = uc(DocumentKind::Csv);
        let info = u.info(&Source::Bytes(vec![1])).unwrap();
        assert_eq!(info.row_count, 3);
        assert_eq!(info.column_count, 2);
    }

    #[test]
    fn non_csv_is_rejected() {
        let u = uc(DocumentKind::Xlsx);
        let err = u.info(&Source::Bytes(vec![1])).unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }

    #[test]
    fn extract_markdown_returns_rows() {
        let u = uc(DocumentKind::Csv);
        let out = u
            .extract_markdown(&Source::Bytes(vec![1]), Some(0..2))
            .unwrap();
        assert_eq!(out.markdown, "| a | b |");
        assert_eq!(out.kind, DocumentKind::Csv);
    }

    #[test]
    fn extract_markdown_char_cap_is_enforced() {
        let u = CsvUseCase::new_with_max_chars(
            Arc::new(FakeClassifier {
                kind: DocumentKind::Csv,
            }),
            Arc::new(FakeReader {
                info: CsvInfo {
                    row_count: 1,
                    column_count: 1,
                    header: vec!["a".into()],
                },
                markdown: "a very long table body".into(),
            }),
            5,
        );
        let err = u
            .extract_markdown(&Source::Bytes(vec![1]), None)
            .unwrap_err();
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));
    }
}
