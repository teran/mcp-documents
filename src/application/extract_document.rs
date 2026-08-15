//! `ExtractDocumentUseCase` — the core routing logic (text vs OCR → merge).

use std::sync::Arc;

use crate::domain::document::{DocumentKind, PageClassification, PdfType};
use crate::domain::error::ExtractionError;
use crate::domain::ports::{DocumentExtractor, OcrProvider, PdfPageRenderer};
use crate::domain::result::{ExtractedPage, ExtractionResult, Source};

/// DPI used when rendering PDF pages to bitmaps for OCR.
pub const OCR_DPI: u32 = 300;

/// Orchestrates document extraction: routes text-based pages to local
/// extraction and scanned/image pages to render + OCR, then merges in order.
///
/// Depends only on the domain ports — never on concrete libraries.
#[derive(Clone)]
pub struct ExtractDocumentUseCase {
    extractor: Arc<dyn DocumentExtractor>,
    ocr: Arc<dyn OcrProvider>,
    renderer: Arc<dyn PdfPageRenderer>,
}

impl ExtractDocumentUseCase {
    /// Build the use case around its injected ports (dependency inversion).
    pub fn new(
        extractor: Arc<dyn DocumentExtractor>,
        ocr: Arc<dyn OcrProvider>,
        renderer: Arc<dyn PdfPageRenderer>,
    ) -> Self {
        Self {
            extractor,
            ocr,
            renderer,
        }
    }

    /// Extract a document, routing pages and merging results.
    pub async fn extract(&self, source: &Source) -> Result<ExtractionResult, ExtractionError> {
        let classification = self.extractor.classify(source)?;

        // Non-PDF kinds (doc/docx/xls/xlsx/ppt/pptx/odf/rtf/epub/csv) go
        // straight through anydoc — no OCR, no rendering.
        if classification.kind != DocumentKind::Pdf {
            return self.extractor.extract_text(source);
        }

        let ocr_pages: Vec<PageClassification> = classification
            .pages_needing_ocr
            .iter()
            .filter(|p| p.needs_ocr)
            .cloned()
            .collect();

        if ocr_pages.is_empty() {
            // Fully text-based: skip OCR entirely, return the local extraction.
            return self.extractor.extract_text(source);
        }

        // Scanned/image pages present: render + OCR them, then merge.
        self.extract_with_ocr(source, classification.pdf_type, &ocr_pages)
            .await
    }

    async fn extract_with_ocr(
        &self,
        source: &Source,
        pdf_type: Option<PdfType>,
        ocr_pages: &[PageClassification],
    ) -> Result<ExtractionResult, ExtractionError> {
        let pdf_bytes = source.read_bytes()?;
        let mut pages = Vec::with_capacity(ocr_pages.len());

        for pc in ocr_pages {
            let idx = pc.page_index;

            // Render the page (CPU-bound) off the async runtime.
            let renderer = self.renderer.clone();
            let bytes = pdf_bytes.clone();
            let image =
                tokio::task::spawn_blocking(move || renderer.render_page(&bytes, idx, OCR_DPI))
                    .await
                    .map_err(|e| ExtractionError::Internal(format!("render task failed: {e}")))??;

            // OCR the rendered bitmap (CPU-bound) off the async runtime. The
            // bitmap carries its real dimensions so OCR geometry is correct.
            let ocr = self.ocr.clone();
            let text = tokio::task::spawn_blocking(move || ocr.ocr_page(&image))
                .await
                .map_err(|e| ExtractionError::Internal(format!("ocr task failed: {e}")))??;

            pages.push(ExtractedPage::ocr(idx, text));
        }

        // Merge preserves page order and records each page's source.
        Ok(ExtractionResult::from_pages(
            DocumentKind::Pdf,
            pdf_type,
            pages,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::document::Classification;
    use crate::domain::result::{PageBitmap, PageSource};

    #[derive(Default)]
    struct FakeExtractor {
        classification: Option<Classification>,
        local_result: Option<ExtractionResult>,
    }

    impl DocumentExtractor for FakeExtractor {
        fn classify(&self, _source: &Source) -> Result<Classification, ExtractionError> {
            self.classification
                .clone()
                .ok_or_else(|| ExtractionError::Internal("no classification".into()))
        }

        fn extract_text(&self, _source: &Source) -> Result<ExtractionResult, ExtractionError> {
            self.local_result
                .clone()
                .ok_or_else(|| ExtractionError::Internal("no local result".into()))
        }
    }

    /// Fake OCR that echoes the page index encoded in the bitmap, records the
    /// dimensions it was given (to assert non-square passthrough), or fails
    /// with `OcrUnavailable` when told to.
    #[derive(Clone)]
    struct FakeOcr {
        unavailable: bool,
        /// (width, height) of the last bitmap passed to `ocr_page`.
        last_dims: std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>>,
    }

    impl FakeOcr {
        fn new(unavailable: bool) -> Self {
            Self {
                unavailable,
                last_dims: std::sync::Arc::new(std::sync::Mutex::new(None)),
            }
        }
    }

    impl OcrProvider for FakeOcr {
        fn ocr_page(&self, bitmap: &PageBitmap) -> Result<String, ExtractionError> {
            *self.last_dims.lock().unwrap() = Some((bitmap.width, bitmap.height));
            if self.unavailable {
                return Err(ExtractionError::OcrUnavailable("no OCR".into()));
            }
            // Bitmap data encodes the page index as the first byte.
            let idx = bitmap.data[0] as usize;
            Ok(format!("ocr-text-{idx}"))
        }
    }

    /// Fake renderer that encodes the page index as the first byte of a
    /// non-square bitmap (e.g. 640x480) to exercise dimension passthrough.
    #[derive(Clone)]
    struct FakeRenderer;

    impl PdfPageRenderer for FakeRenderer {
        fn render_page(
            &self,
            _pdf_bytes: &[u8],
            page_index: usize,
            _dpi: u32,
        ) -> Result<PageBitmap, ExtractionError> {
            // Non-square page (640x480) with a leading page-index marker.
            PageBitmap::new(640, 480, 1, vec![page_index as u8; 640 * 480])
        }
    }

    fn classification_with(needs_ocr: &[usize]) -> Classification {
        let pages = (0..3)
            .map(|i| PageClassification::new(i, needs_ocr.contains(&i), 0.9))
            .collect();
        Classification {
            kind: DocumentKind::Pdf,
            pdf_type: Some(PdfType::Mixed),
            pages_needing_ocr: pages,
        }
    }

    fn setup(needs_ocr: &[usize]) -> ExtractDocumentUseCase {
        let extractor = FakeExtractor {
            classification: Some(classification_with(needs_ocr)),
            local_result: Some(ExtractionResult::markdown_only(
                DocumentKind::Pdf,
                Some(PdfType::TextBased),
                "local body".into(),
            )),
        };
        ExtractDocumentUseCase::new(
            Arc::new(extractor),
            Arc::new(FakeOcr::new(false)),
            Arc::new(FakeRenderer),
        )
    }

    #[tokio::test]
    async fn non_pdf_routes_to_extract_text_without_ocr() {
        // Classified as a non-PDF kind: must go straight to extract_text and
        // never touch render/OCR (which would fail here if called).
        let extractor = FakeExtractor {
            classification: Some(Classification {
                kind: DocumentKind::Docx,
                pdf_type: None,
                pages_needing_ocr: Vec::new(),
            }),
            local_result: Some(ExtractionResult::markdown_only(
                DocumentKind::Docx,
                None,
                "docx body".into(),
            )),
        };
        let uc = ExtractDocumentUseCase::new(
            Arc::new(extractor),
            Arc::new(FakeOcr::new(false)),
            Arc::new(FakeRenderer),
        );
        let out = uc.extract(&Source::Bytes(vec![9, 9, 9])).await.unwrap();
        assert_eq!(out.markdown, "docx body");
        assert_eq!(out.kind, DocumentKind::Docx);
        assert!(out.pages.is_empty());
    }

    #[tokio::test]
    async fn text_based_document_skips_ocr() {
        let uc = setup(&[]);
        let src = Source::Bytes(vec![9, 9, 9]);
        let out = uc.extract(&src).await.unwrap();
        assert_eq!(out.markdown, "local body");
        assert!(out.pages.is_empty());
        assert_eq!(out.pdf_type, Some(PdfType::TextBased));
    }

    #[tokio::test]
    async fn scanned_pages_are_rendered_and_ocred_in_order() {
        let uc = setup(&[1, 2]);
        let src = Source::Bytes(vec![9, 9, 9]);
        let out = uc.extract(&src).await.unwrap();

        // Only OCR pages are produced, in ascending page order.
        assert_eq!(out.pages.len(), 2);
        assert_eq!(out.pages[0].page_index, 1);
        assert_eq!(out.pages[0].source, PageSource::Ocr);
        assert_eq!(out.pages[0].text, "ocr-text-1");
        assert_eq!(out.pages[1].page_index, 2);
        assert_eq!(out.pages[1].text, "ocr-text-2");
        // Merged body preserves order.
        assert_eq!(out.markdown, "ocr-text-1\n\nocr-text-2");
    }

    #[tokio::test]
    async fn non_square_bitmap_dimensions_pass_through_to_ocr() {
        let extractor = FakeExtractor {
            classification: Some(classification_with(&[0])),
            local_result: None,
        };
        let ocr = FakeOcr::new(false);
        let uc = ExtractDocumentUseCase::new(
            Arc::new(extractor),
            Arc::new(ocr.clone()),
            Arc::new(FakeRenderer),
        );
        let out = uc.extract(&Source::Bytes(vec![9])).await.unwrap();
        assert_eq!(out.pages[0].text, "ocr-text-0");
        // The fake renderer produces a 640x480 (non-square) bitmap; OCR must
        // have been given those exact dimensions, not a square guess.
        assert_eq!(*ocr.last_dims.lock().unwrap(), Some((640, 480)));
    }

    #[tokio::test]
    async fn ocr_unavailable_returns_clear_error() {
        let extractor = FakeExtractor {
            classification: Some(classification_with(&[0])),
            local_result: None,
        };
        let uc = ExtractDocumentUseCase::new(
            Arc::new(extractor),
            Arc::new(FakeOcr::new(true)),
            Arc::new(FakeRenderer),
        );
        let err = uc.extract(&Source::Bytes(vec![9])).await.unwrap_err();
        assert!(matches!(err, ExtractionError::OcrUnavailable(_)));
    }

    #[tokio::test]
    async fn path_source_read_for_ocr() {
        // A real temp file on disk exercises Source::read_bytes.
        let dir = std::env::temp_dir().join(format!("mcp-doc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.bin");
        std::fs::write(&path, b"fake pdf bytes").unwrap();

        let uc = setup(&[0]);
        let out = uc.extract(&Source::Path(path.clone())).await.unwrap();
        assert_eq!(out.pages[0].text, "ocr-text-0");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn io_error_on_missing_path() {
        let uc = setup(&[0]);
        let err = uc
            .extract(&Source::Path(std::path::PathBuf::from(
                "/definitely/missing.pdf",
            )))
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Io(_)));
    }
}
