//! `PdfUseCase` — PDF metadata + per-page extraction with real per-page text
//! and OCR routing.

use std::ops::Range;
use std::sync::Arc;

use crate::config::{ocr_concurrency, ocr_max_pages};
use crate::domain::document::{DocumentKind, PageClassification};
use crate::domain::error::ExtractionError;
use crate::domain::meta::PdfInfo;
use crate::domain::ports::{DocumentExtractor, OcrProvider, PdfPageRenderer, PdfTextSource};
use crate::domain::ranges::resolve_range;
use crate::domain::result::{ExtractedPage, ExtractionResult, Source};

/// DPI used when rendering PDF pages to bitmaps for OCR.
pub const OCR_DPI: u32 = 300;

/// Orchestrates PDF metadata and extraction: routes text pages to the native
/// text layer (`pdf-inspector`) and scanned pages to render + OCR, then merges
/// in page order. Enforces the OCR page cap and bounded OCR concurrency.
#[derive(Clone)]
pub struct PdfUseCase {
    classifier: Arc<dyn DocumentExtractor>,
    text_source: Arc<dyn PdfTextSource>,
    renderer: Arc<dyn PdfPageRenderer>,
    ocr: Arc<dyn OcrProvider>,
    /// Cap on the number of OCR pages processed per request (`OCR_MAX_PAGES`).
    max_ocr_pages: usize,
    /// Max parallel OCR pages per request (`OCR_CONCURRENCY`).
    ocr_concurrency: usize,
}

impl PdfUseCase {
    /// Build the use case around its injected ports, reading limits from env.
    pub fn new(
        classifier: Arc<dyn DocumentExtractor>,
        text_source: Arc<dyn PdfTextSource>,
        renderer: Arc<dyn PdfPageRenderer>,
        ocr: Arc<dyn OcrProvider>,
    ) -> Self {
        Self::new_with_limits(
            classifier,
            text_source,
            renderer,
            ocr,
            ocr_max_pages(),
            ocr_concurrency(),
        )
    }

    /// Build with explicit OCR limits (used by tests to avoid env races).
    pub fn new_with_limits(
        classifier: Arc<dyn DocumentExtractor>,
        text_source: Arc<dyn PdfTextSource>,
        renderer: Arc<dyn PdfPageRenderer>,
        ocr: Arc<dyn OcrProvider>,
        max_ocr_pages: usize,
        ocr_concurrency: usize,
    ) -> Self {
        Self {
            classifier,
            text_source,
            renderer,
            ocr,
            max_ocr_pages,
            ocr_concurrency,
        }
    }

    /// Verify the source is actually a PDF, returning a clear tool-level error
    /// otherwise.
    fn require_pdf(&self, source: &Source) -> Result<(), ExtractionError> {
        let c = self.classifier.classify(source)?;
        if c.kind != DocumentKind::Pdf {
            return Err(ExtractionError::Unsupported(c.kind));
        }
        Ok(())
    }

    /// PDF metadata (page count, type, per-page OCR needs).
    pub fn info(&self, source: &Source) -> Result<PdfInfo, ExtractionError> {
        self.require_pdf(source)?;
        self.text_source.pdf_info(source)
    }

    /// Extract the requested 0-based page range (default all pages) as Markdown,
    /// merging native text pages with OCR'd scanned pages in page order.
    pub async fn extract_markdown(
        &self,
        source: &Source,
        page_range: Option<Range<usize>>,
    ) -> Result<ExtractionResult, ExtractionError> {
        self.require_pdf(source)?;
        let info = self.text_source.pdf_info(source)?;

        let from = page_range.as_ref().map(|r| r.start);
        let to = page_range.as_ref().map(|r| r.end);
        let range = resolve_range(from, to, info.page_count, "page")?;

        let mut pages: Vec<ExtractedPage> = Vec::new();

        // Separate the pages in range into native-text vs OCR-needed.
        let needs_ocr: Vec<bool> = {
            let mut v = vec![false; info.page_count];
            for pc in &info.pages_needing_ocr {
                if pc.needs_ocr {
                    v[pc.page_index] = true;
                }
            }
            v
        };
        let mut text_pages: Vec<usize> = Vec::new();
        let mut ocr_pages: Vec<usize> = Vec::new();
        for idx in range.clone() {
            if needs_ocr.get(idx).copied().unwrap_or(false) {
                ocr_pages.push(idx);
            } else {
                text_pages.push(idx);
            }
        }

        // OCR page cap per request.
        if ocr_pages.len() > self.max_ocr_pages {
            return Err(ExtractionError::ResourceLimit(format!(
                "request requires OCR on {} pages, exceeding OCR_MAX_PAGES={}",
                ocr_pages.len(),
                self.max_ocr_pages
            )));
        }

        // Native text layer for text pages.
        if !text_pages.is_empty() {
            pages.extend(self.text_source.extract_pages(source, &text_pages)?);
        }

        // Render + OCR scanned pages concurrently, bounded by OCR_CONCURRENCY.
        if !ocr_pages.is_empty() {
            // Share the PDF bytes across tasks via Arc instead of cloning the
            // whole buffer once per page (peak memory stays near file size, not
            // pages × file size).
            let pdf_bytes: Arc<[u8]> = Arc::from(source.read_bytes()?);
            let mut ocr_results = self.ocr_pages_concurrently(pdf_bytes, &ocr_pages).await?;
            pages.append(&mut ocr_results);
        }

        // If the range is empty, produce an empty-but-valid result.
        if pages.is_empty() && range.is_empty() {
            return Ok(ExtractionResult::markdown_only(
                DocumentKind::Pdf,
                Some(info.pdf_type),
                String::new(),
            ));
        }

        // Keep the per-page breakdown in ascending page order.
        pages.sort_by_key(|p| p.page_index);

        Ok(ExtractionResult::from_pages(
            DocumentKind::Pdf,
            Some(info.pdf_type),
            pages,
        ))
    }

    /// Render + OCR the given pages concurrently, bounded by a semaphore sized
    /// to `OCR_CONCURRENCY`. Each page's render and OCR run on the blocking
    /// thread pool. `pdf_bytes` is shared across tasks via `Arc` (no per-page
    /// clone of the whole buffer).
    async fn ocr_pages_concurrently(
        &self,
        pdf_bytes: Arc<[u8]>,
        pages: &[usize],
    ) -> Result<Vec<ExtractedPage>, ExtractionError> {
        // Clamp to a minimum of 1 permit: a mutated/boundary `ocr_concurrency`
        // of 0 would create a 0-permit semaphore, `acquire_owned()` would never
        // resolve, and the OCR pipeline would deadlock until the per-mutant
        // timeout. Guard here so OCR always makes forward progress.
        let sem = Arc::new(tokio::sync::Semaphore::new(self.ocr_concurrency.max(1)));
        let mut set = tokio::task::JoinSet::new();

        for &idx in pages {
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| ExtractionError::Internal(format!("ocr semaphore: {e}")))?;
            let renderer = self.renderer.clone();
            let ocr = self.ocr.clone();
            let bytes = pdf_bytes.clone();
            set.spawn(async move {
                let _permit = permit;
                // Render (CPU-bound) off the async runtime.
                let image = tokio::task::spawn_blocking(move || {
                    renderer.render_page(bytes.as_ref(), idx, OCR_DPI)
                })
                .await
                .map_err(|e| ExtractionError::Internal(format!("render task failed: {e}")))??;
                // OCR (CPU-bound) off the async runtime.
                let text = tokio::task::spawn_blocking(move || ocr.ocr_page(&image))
                    .await
                    .map_err(|e| ExtractionError::Internal(format!("ocr task failed: {e}")))??;
                Ok::<ExtractedPage, ExtractionError>(ExtractedPage::ocr(idx, text))
            });
        }

        let mut results = Vec::with_capacity(pages.len());
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(page)) => results.push(page),
                Ok(Err(e)) => return Err(e),
                Err(e) => {
                    return Err(ExtractionError::Internal(format!("ocr task panicked: {e}")));
                }
            }
        }
        Ok(results)
    }
}

/// Convenience: extract a range of pages and merge them (pure helper, testable).
pub fn pages_in_range(needs_ocr: &[PageClassification], range: Range<usize>) -> Vec<usize> {
    range
        .filter(|&i| needs_ocr.iter().any(|p| p.page_index == i && p.needs_ocr))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::document::Classification;
    use crate::domain::document::PdfType;
    use crate::domain::result::{PageBitmap, PageSource};

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
            unreachable!("not used in pdf tests")
        }
    }

    #[derive(Clone)]
    struct FakeTextSource {
        info: PdfInfo,
    }
    impl PdfTextSource for FakeTextSource {
        fn pdf_info(&self, _source: &Source) -> Result<PdfInfo, ExtractionError> {
            Ok(self.info.clone())
        }
        fn extract_pages(
            &self,
            _source: &Source,
            pages: &[usize],
        ) -> Result<Vec<ExtractedPage>, ExtractionError> {
            Ok(pages
                .iter()
                .map(|&p| ExtractedPage::local(p, format!("text-{p}")))
                .collect())
        }
    }

    #[derive(Clone)]
    struct FakeRenderer;
    impl PdfPageRenderer for FakeRenderer {
        fn render_page(
            &self,
            _pdf: &[u8],
            page_index: usize,
            _dpi: u32,
        ) -> Result<PageBitmap, ExtractionError> {
            PageBitmap::new(16, 16, 1, vec![page_index as u8; 16 * 16])
        }
    }

    #[derive(Clone)]
    struct FakeOcr;
    impl OcrProvider for FakeOcr {
        fn ocr_page(&self, bitmap: &PageBitmap) -> Result<String, ExtractionError> {
            Ok(format!("ocr-{}", bitmap.data[0]))
        }
    }

    fn info_with(needs_ocr: &[usize]) -> PdfInfo {
        PdfInfo {
            page_count: 4,
            pdf_type: PdfType::Mixed,
            pages_needing_ocr: needs_ocr
                .iter()
                .map(|&i| PageClassification::new(i, true, 1.0))
                .collect(),
        }
    }

    fn uc(info: PdfInfo, kind: DocumentKind) -> PdfUseCase {
        PdfUseCase::new(
            Arc::new(FakeClassifier { kind }),
            Arc::new(FakeTextSource { info }),
            Arc::new(FakeRenderer),
            Arc::new(FakeOcr),
        )
    }

    #[tokio::test]
    async fn non_pdf_is_rejected() {
        let u = uc(info_with(&[]), DocumentKind::Docx);
        let err = u
            .extract_markdown(&Source::Bytes(vec![1]), None)
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }

    #[tokio::test]
    async fn info_rejects_non_pdf() {
        let u = uc(info_with(&[]), DocumentKind::Pptx);
        let err = u.info(&Source::Bytes(vec![1])).unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }

    #[tokio::test]
    async fn text_only_pdf_uses_local_pages() {
        let u = uc(info_with(&[]), DocumentKind::Pdf);
        let out = u
            .extract_markdown(&Source::Bytes(vec![9]), None)
            .await
            .unwrap();
        assert_eq!(out.markdown, "text-0\n\ntext-1\n\ntext-2\n\ntext-3");
        assert!(out.pages.iter().all(|p| p.source == PageSource::Local));
        assert_eq!(out.pdf_type, Some(PdfType::Mixed));
    }

    #[tokio::test]
    async fn mixed_pdf_merges_local_and_ocr_in_order() {
        let u = uc(info_with(&[1, 3]), DocumentKind::Pdf);
        let out = u
            .extract_markdown(&Source::Bytes(vec![9]), None)
            .await
            .unwrap();
        // Page order: 0 text, 1 ocr, 2 text, 3 ocr.
        assert_eq!(out.markdown, "text-0\n\nocr-1\n\ntext-2\n\nocr-3");
        assert_eq!(out.pages.len(), 4);
        assert_eq!(out.pages[1].source, PageSource::Ocr);
        assert_eq!(out.pages[1].text, "ocr-1");
    }

    #[tokio::test]
    async fn page_range_extracts_subset() {
        let u = uc(info_with(&[1, 3]), DocumentKind::Pdf);
        let out = u
            .extract_markdown(&Source::Bytes(vec![9]), Some(1..3))
            .await
            .unwrap();
        assert_eq!(out.markdown, "ocr-1\n\ntext-2");
    }

    #[tokio::test]
    #[allow(clippy::reversed_empty_ranges)]
    async fn from_greater_than_to_is_malformed() {
        let u = uc(info_with(&[]), DocumentKind::Pdf);
        let err = u
            .extract_markdown(&Source::Bytes(vec![9]), Some(3..1))
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Malformed(_)));
    }

    #[tokio::test]
    async fn ocr_page_cap_is_enforced() {
        // All 4 pages need OCR; cap is 3.
        let u = PdfUseCase::new_with_limits(
            Arc::new(FakeClassifier {
                kind: DocumentKind::Pdf,
            }),
            Arc::new(FakeTextSource {
                info: info_with(&[0, 1, 2, 3]),
            }),
            Arc::new(FakeRenderer),
            Arc::new(FakeOcr),
            3,
            2,
        );
        let err = u
            .extract_markdown(&Source::Bytes(vec![9]), None)
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));
    }

    #[tokio::test]
    async fn ocr_concurrency_is_respected() {
        // 4 pages need OCR, concurrency limited to 1; should still work.
        let u = PdfUseCase::new_with_limits(
            Arc::new(FakeClassifier {
                kind: DocumentKind::Pdf,
            }),
            Arc::new(FakeTextSource {
                info: info_with(&[0, 1, 2, 3]),
            }),
            Arc::new(FakeRenderer),
            Arc::new(FakeOcr),
            100,
            1,
        );
        let out = u
            .extract_markdown(&Source::Bytes(vec![9]), None)
            .await
            .unwrap();
        assert_eq!(out.pages.len(), 4);
        assert_eq!(out.markdown, "ocr-0\n\nocr-1\n\nocr-2\n\nocr-3");
    }

    #[tokio::test]
    async fn ocr_concurrency_of_zero_does_not_hang() {
        // Regression guard: a mutated/boundary `ocr_concurrency` of 0 must NOT
        // deadlock the pipeline (a 0-permit semaphore would make acquire_owned()
        // never resolve). The semaphore is clamped to >= 1, so all 4 OCR pages
        // complete normally.
        let u = PdfUseCase::new_with_limits(
            Arc::new(FakeClassifier {
                kind: DocumentKind::Pdf,
            }),
            Arc::new(FakeTextSource {
                info: info_with(&[0, 1, 2, 3]),
            }),
            Arc::new(FakeRenderer),
            Arc::new(FakeOcr),
            100,
            0,
        );
        let out = u
            .extract_markdown(&Source::Bytes(vec![9]), None)
            .await
            .unwrap();
        assert_eq!(out.pages.len(), 4);
        assert_eq!(out.markdown, "ocr-0\n\nocr-1\n\nocr-2\n\nocr-3");
    }

    #[test]
    fn pages_in_range_filters_needing_ocr() {
        let pc = vec![
            PageClassification::new(0, false, 0.9),
            PageClassification::new(1, true, 0.9),
            PageClassification::new(2, true, 0.9),
        ];
        assert_eq!(pages_in_range(&pc, 0..3), vec![1, 2]);
    }

    #[tokio::test]
    async fn empty_page_range_returns_empty_markdown() {
        // A page range that resolves to zero pages yields an empty-but-valid
        // result (the markdown_only early return).
        let u = uc(info_with(&[]), DocumentKind::Pdf);
        let out = u
            .extract_markdown(&Source::Bytes(vec![9]), Some(4..4))
            .await
            .unwrap();
        assert_eq!(out.markdown, "");
    }

    #[tokio::test]
    async fn empty_range_returns_empty_result() {
        let u = uc(info_with(&[0, 1, 2, 3]), DocumentKind::Pdf);
        let out = u
            .extract_markdown(&Source::Bytes(vec![9]), Some(0..0))
            .await
            .unwrap();
        assert!(out.markdown.is_empty());
        assert!(out.pages.is_empty());
    }

    struct FailingOcr;
    impl OcrProvider for FailingOcr {
        fn ocr_page(&self, _b: &PageBitmap) -> Result<String, ExtractionError> {
            Err(ExtractionError::OcrUnavailable("no ocr".into()))
        }
    }

    #[tokio::test]
    async fn ocr_error_is_propagated() {
        // An OCR failure inside the concurrent pipeline must surface as the
        // underlying domain error (the Ok(Err) join branch).
        let u = PdfUseCase::new_with_limits(
            Arc::new(FakeClassifier {
                kind: DocumentKind::Pdf,
            }),
            Arc::new(FakeTextSource {
                info: info_with(&[0]),
            }),
            Arc::new(FakeRenderer),
            Arc::new(FailingOcr),
            100,
            2,
        );
        let err = u
            .extract_markdown(&Source::Bytes(vec![9]), None)
            .await
            .unwrap_err();
        assert!(matches!(err, ExtractionError::OcrUnavailable(_)));
    }
}
