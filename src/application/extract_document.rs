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

        // Does the PDF also contain text-based pages (i.e. is it Mixed)? If
        // every page is scanned there is no text layer to preserve.
        let has_text_pages = classification
            .pages_needing_ocr
            .iter()
            .any(|p| !p.needs_ocr);

        // Scanned/image pages present: render + OCR them, then merge with any
        // text-layer content.
        self.extract_with_ocr(source, classification.pdf_type, &ocr_pages, has_text_pages)
            .await
    }

    async fn extract_with_ocr(
        &self,
        source: &Source,
        pdf_type: Option<PdfType>,
        ocr_pages: &[PageClassification],
        has_text_pages: bool,
    ) -> Result<ExtractionResult, ExtractionError> {
        let pdf_bytes = source.read_bytes()?;
        let mut ocr_entries = Vec::with_capacity(ocr_pages.len());

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

            ocr_entries.push(ExtractedPage::ocr(idx, text));
        }

        // For a Mixed PDF also preserve the text-based layer. anydoc returns the
        // whole document's Markdown (not per-page), so exact page interleaving
        // is unavailable — we keep the text layer alongside the OCR'd pages.
        let local_markdown = if has_text_pages {
            let local = self.extractor.extract_text(source)?;
            local.markdown
        } else {
            String::new()
        };

        Ok(merge_mixed(
            DocumentKind::Pdf,
            pdf_type,
            &local_markdown,
            ocr_entries,
        ))
    }
}

/// Merge the local (text-layer) content with OCR'd pages for a mixed PDF.
///
/// # Limitation
/// `anydoc` returns the **whole document's** Markdown rather than per-page
/// text, so exact page-by-page interleaving of the text layer with the OCR'd
/// pages is not available in this pipeline. This function produces a
/// pragmatic, coherent result: the text-layer Markdown is placed first,
/// followed by the OCR'd pages (in ascending page order), and each OCR'd page
/// is recorded individually with `PageSource::Ocr`. The text layer is
/// represented in the merged body (it has no per-page breakdown from `anydoc`),
/// so it is not emitted as individual `ExtractedPage` entries.
pub fn merge_mixed(
    kind: DocumentKind,
    pdf_type: Option<PdfType>,
    local_markdown: &str,
    ocr_pages: Vec<ExtractedPage>,
) -> ExtractionResult {
    let mut parts: Vec<String> = Vec::new();
    let local = local_markdown.trim();
    if !local.is_empty() {
        parts.push(local.to_string());
    }

    let mut ocr_sorted = ocr_pages.clone();
    ocr_sorted.sort_by_key(|p| p.page_index);
    for p in &ocr_sorted {
        let t = p.text.trim();
        if !t.is_empty() {
            parts.push(t.to_string());
        }
    }

    ExtractionResult {
        kind,
        pdf_type,
        markdown: parts.join("\n\n"),
        pages: ocr_pages,
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
        // Merged body: text layer (page 0 is text) then OCR pages.
        assert_eq!(out.markdown, "local body\n\nocr-text-1\n\nocr-text-2");
    }

    #[tokio::test]
    async fn mixed_pdf_keeps_text_layer_and_ocr_pages() {
        // Pages 0 and 2 are text-based; pages 1 and 3 need OCR -> Mixed.
        let extractor = FakeExtractor {
            classification: Some(Classification {
                kind: DocumentKind::Pdf,
                pdf_type: Some(PdfType::Mixed),
                pages_needing_ocr: (0..4)
                    .map(|i| PageClassification::new(i, i == 1 || i == 3, 0.9))
                    .collect(),
            }),
            local_result: Some(ExtractionResult::markdown_only(
                DocumentKind::Pdf,
                Some(PdfType::Mixed),
                "text layer content".into(),
            )),
        };
        let uc = ExtractDocumentUseCase::new(
            Arc::new(extractor),
            Arc::new(FakeOcr::new(false)),
            Arc::new(FakeRenderer),
        );
        let out = uc.extract(&Source::Bytes(vec![9])).await.unwrap();

        // BOTH the text-layer content and the OCR'd pages must be present, in
        // a coherent merged body (text layer first, then OCR pages in order).
        assert!(out.markdown.contains("text layer content"));
        assert!(out.markdown.contains("ocr-text-1"));
        assert!(out.markdown.contains("ocr-text-3"));
        // OCR pages are recorded per-page with PageSource::Ocr.
        assert_eq!(out.pages.len(), 2);
        assert!(out.pages.iter().all(|p| p.source == PageSource::Ocr));
        assert_eq!(
            out.markdown,
            "text layer content\n\nocr-text-1\n\nocr-text-3"
        );
    }

    #[tokio::test]
    async fn fully_scanned_pdf_uses_only_ocr() {
        // Every page needs OCR -> no text layer to preserve.
        let extractor = FakeExtractor {
            classification: Some(Classification {
                kind: DocumentKind::Pdf,
                pdf_type: Some(PdfType::Scanned),
                pages_needing_ocr: (0..3)
                    .map(|i| PageClassification::new(i, true, 1.0))
                    .collect(),
            }),
            local_result: Some(ExtractionResult::markdown_only(
                DocumentKind::Pdf,
                Some(PdfType::Scanned),
                "should not appear".into(),
            )),
        };
        let uc = ExtractDocumentUseCase::new(
            Arc::new(extractor),
            Arc::new(FakeOcr::new(false)),
            Arc::new(FakeRenderer),
        );
        let out = uc.extract(&Source::Bytes(vec![9])).await.unwrap();

        assert_eq!(out.markdown, "ocr-text-0\n\nocr-text-1\n\nocr-text-2");
        assert!(!out.markdown.contains("should not appear"));
        assert_eq!(out.pages.len(), 3);
    }

    #[test]
    fn merge_mixed_prepends_text_layer_and_orders_ocr() {
        let ocr_pages = vec![
            ExtractedPage::ocr(2, "ocr-two"),
            ExtractedPage::ocr(0, "ocr-zero"),
        ];
        let out = merge_mixed(
            DocumentKind::Pdf,
            Some(PdfType::Mixed),
            "text layer",
            ocr_pages,
        );
        // Text layer first, then OCR pages in page order (0 before 2).
        assert_eq!(out.markdown, "text layer\n\nocr-zero\n\nocr-two");
        // OCR pages recorded per-page.
        assert_eq!(out.pages.len(), 2);
        assert_eq!(out.pages[0].page_index, 2);
    }

    #[test]
    fn merge_mixed_without_text_layer_just_joins_ocr() {
        let out = merge_mixed(
            DocumentKind::Pdf,
            Some(PdfType::Scanned),
            "   ",
            vec![ExtractedPage::ocr(0, "only-ocr")],
        );
        assert_eq!(out.markdown, "only-ocr");
    }

    #[tokio::test]
    async fn non_square_bitmap_dimensions_pass_through_to_ocr() {
        let extractor = FakeExtractor {
            classification: Some(classification_with(&[0])),
            local_result: Some(ExtractionResult::markdown_only(
                DocumentKind::Pdf,
                Some(PdfType::Mixed),
                "local".into(),
            )),
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
