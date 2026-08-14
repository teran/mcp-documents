//! `ClassifyDocumentUseCase` — detect a document's kind and PDF type.

use std::sync::Arc;

use crate::domain::document::Classification;
use crate::domain::error::ExtractionError;
use crate::domain::ports::DocumentExtractor;
use crate::domain::result::Source;

/// Classifies a document by delegating to the injected [`DocumentExtractor`].
#[derive(Clone)]
pub struct ClassifyDocumentUseCase {
    extractor: Arc<dyn DocumentExtractor>,
}

impl ClassifyDocumentUseCase {
    /// Build the use case around a document extractor (dependency inversion).
    pub fn new(extractor: Arc<dyn DocumentExtractor>) -> Self {
        Self { extractor }
    }

    /// Classify a document from its source.
    pub fn classify(&self, source: &Source) -> Result<Classification, ExtractionError> {
        self.extractor.classify(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::document::{DocumentKind, PageClassification, PdfType};
    use crate::domain::ports::DocumentExtractor;

    /// In-memory fake extractor returning a canned classification.
    #[derive(Default)]
    struct FakeExtractor {
        result: Option<Classification>,
        error: Option<ExtractionError>,
    }

    impl DocumentExtractor for FakeExtractor {
        fn classify(&self, _source: &Source) -> Result<Classification, ExtractionError> {
            match (&self.result, &self.error) {
                (Some(r), _) => Ok(r.clone()),
                (None, Some(e)) => Err(e.clone()),
                _ => Err(ExtractionError::Internal("no stub configured".into())),
            }
        }

        fn extract_text(
            &self,
            _source: &Source,
        ) -> Result<crate::domain::result::ExtractionResult, ExtractionError> {
            unreachable!("not exercised in classify tests")
        }
    }

    fn classification() -> Classification {
        Classification {
            kind: DocumentKind::Pdf,
            pdf_type: Some(PdfType::Mixed),
            pages_needing_ocr: vec![PageClassification::new(0, true, 0.9)],
        }
    }

    #[test]
    fn classify_delegates_to_extractor() {
        let fake = FakeExtractor {
            result: Some(classification()),
            error: None,
        };
        let uc = ClassifyDocumentUseCase::new(Arc::new(fake));
        let out = uc.classify(&Source::Bytes(vec![1, 2, 3])).unwrap();
        assert_eq!(out.kind, DocumentKind::Pdf);
        assert_eq!(out.pdf_type(), Some(PdfType::Mixed));
        assert_eq!(out.ocr_page_count(), 1);
    }

    #[test]
    fn classify_propagates_error() {
        let fake = FakeExtractor {
            result: None,
            error: Some(ExtractionError::Malformed("nope".into())),
        };
        let uc = ClassifyDocumentUseCase::new(Arc::new(fake));
        let err = uc.classify(&Source::Bytes(vec![])).unwrap_err();
        assert!(matches!(err, ExtractionError::Malformed(_)));
    }
}
