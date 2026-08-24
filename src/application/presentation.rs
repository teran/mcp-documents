//! `PresentationUseCase` — presentations (ppt / pptx): slide count + slide
//! extraction.

use std::ops::Range;
use std::sync::Arc;

use crate::domain::document::DocumentKind;
use crate::domain::error::ExtractionError;
use crate::domain::meta::PresentationInfo;
use crate::domain::ports::{DocumentExtractor, PresentationReader};
use crate::domain::result::{ExtractionResult, Source};

/// Orchestrates presentation metadata and extraction.
#[derive(Clone)]
pub struct PresentationUseCase {
    classifier: Arc<dyn DocumentExtractor>,
    reader: Arc<dyn PresentationReader>,
}

impl PresentationUseCase {
    /// Build the use case around its injected ports.
    pub fn new(
        classifier: Arc<dyn DocumentExtractor>,
        reader: Arc<dyn PresentationReader>,
    ) -> Self {
        Self { classifier, reader }
    }

    /// Verify the source is exactly `expected` (`Ppt` or `Pptx`), returning a
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

    /// Slide count (1 for legacy `.ppt`).
    pub fn info(
        &self,
        source: &Source,
        expected: DocumentKind,
    ) -> Result<PresentationInfo, ExtractionError> {
        let _kind = self.require_kind(source, expected)?;
        self.reader.presentation_info(source)
    }

    /// Extract the requested 0-based `slide_range` as Markdown (ignored for
    /// legacy `.ppt`, which returns the whole document).
    pub fn extract_markdown(
        &self,
        source: &Source,
        expected: DocumentKind,
        slide_range: Option<Range<usize>>,
    ) -> Result<ExtractionResult, ExtractionError> {
        let kind = self.require_kind(source, expected)?;
        let markdown = self.reader.extract_presentation(source, slide_range)?;
        Ok(ExtractionResult::markdown_only(kind, None, markdown))
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
        slides: usize,
        markdown: String,
    }
    impl PresentationReader for FakeReader {
        fn presentation_info(&self, _source: &Source) -> Result<PresentationInfo, ExtractionError> {
            Ok(PresentationInfo {
                slide_count: self.slides,
            })
        }
        fn extract_presentation(
            &self,
            _source: &Source,
            _slide_range: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok(self.markdown.clone())
        }
    }

    fn uc(kind: DocumentKind) -> PresentationUseCase {
        PresentationUseCase::new(
            Arc::new(FakeClassifier { kind }),
            Arc::new(FakeReader {
                slides: 3,
                markdown: "slide text".into(),
            }),
        )
    }

    #[test]
    fn info_returns_slide_count() {
        let u = uc(DocumentKind::Pptx);
        let info = u.info(&Source::Bytes(vec![1]), DocumentKind::Pptx).unwrap();
        assert_eq!(info.slide_count, 3);
    }

    #[test]
    fn non_presentation_is_rejected() {
        let u = uc(DocumentKind::Xlsx);
        let err = u
            .info(&Source::Bytes(vec![1]), DocumentKind::Pptx)
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }

    #[test]
    fn extract_markdown_returns_slides() {
        let u = uc(DocumentKind::Ppt);
        let out = u
            .extract_markdown(&Source::Bytes(vec![1]), DocumentKind::Ppt, Some(0..2))
            .unwrap();
        assert_eq!(out.markdown, "slide text");
        assert_eq!(out.kind, DocumentKind::Ppt);
    }
}
