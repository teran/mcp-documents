//! `CsvUseCase` — comma-separated values: streaming info + row-range extraction.

use std::ops::Range;
use std::sync::Arc;

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
}

impl CsvUseCase {
    /// Build the use case around its injected ports.
    pub fn new(classifier: Arc<dyn DocumentExtractor>, reader: Arc<dyn CsvReader>) -> Self {
        Self { classifier, reader }
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
    pub fn extract_markdown(
        &self,
        source: &Source,
        row_range: Option<Range<usize>>,
    ) -> Result<ExtractionResult, ExtractionError> {
        self.require_csv(source)?;
        let markdown = self.reader.extract_csv(source, row_range)?;
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
}
