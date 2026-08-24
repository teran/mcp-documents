//! `PdfTextSourceAdapter` — implements [`PdfTextSource`] using `pdf-inspector`.
//!
//! `pdf-inspector` yields **real per-page** results: `detect_pdf_type_mem_with_config`
//! classifies each page (so `Mixed`/`Scanned`/`ImageBased`/`TextBased` are
//! accurate and page ranges work), and `extract_text_with_positions_mem_pages`
//! returns position-tagged text items that we group into per-page text.

use std::collections::HashMap;
use std::sync::Arc;

use pdf_inspector::{DetectionConfig, ScanStrategy};

use crate::domain::document::PageClassification;
use crate::domain::error::ExtractionError;
use crate::domain::meta::PdfInfo;
use crate::domain::ports::PdfTextSource;
use crate::domain::result::{ExtractedPage, Source};

use super::anydoc_extractor::map_pdf_inspector_type;
use super::error_map::map_pdf_inspector_error;
use super::file_reader::read_source_bytes;

/// Adapter providing per-page PDF text-layer extraction.
#[derive(Default)]
pub struct PdfTextSourceAdapter {}

impl PdfTextSourceAdapter {
    /// Create a new adapter.
    pub fn new() -> Self {
        Self {}
    }
}

impl PdfTextSource for PdfTextSourceAdapter {
    fn pdf_info(&self, source: &Source) -> Result<PdfInfo, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let config = DetectionConfig {
            strategy: ScanStrategy::Full,
            ..Default::default()
        };
        let result = std::panic::catch_unwind(|| {
            pdf_inspector::detect_pdf_type_mem_with_config(&bytes, config)
        });
        let result = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(map_pdf_inspector_error(e)),
            Err(_) => {
                return Err(ExtractionError::Internal(
                    "PDF detector panicked while reading info".into(),
                ));
            }
        };

        let pages = result
            .pages_needing_ocr
            .iter()
            .map(|&p| PageClassification::new(p as usize - 1, true, 1.0))
            .collect();
        Ok(PdfInfo {
            page_count: result.page_count as usize,
            pdf_type: map_pdf_inspector_type(result.pdf_type),
            pages_needing_ocr: pages,
        })
    }

    fn extract_pages(
        &self,
        source: &Source,
        pages: &[usize],
    ) -> Result<Vec<ExtractedPage>, ExtractionError> {
        if pages.is_empty() {
            return Ok(Vec::new());
        }
        let bytes = read_source_bytes(source)?;

        // pdf-inspector filters are 1-indexed.
        let filter: std::collections::HashSet<u32> = pages.iter().map(|&p| p as u32 + 1).collect();
        let items = std::panic::catch_unwind(|| {
            pdf_inspector::extractor::extract_text_with_positions_mem_pages(&bytes, Some(&filter))
        });
        let items = match items {
            Ok(Ok(items)) => items,
            Ok(Err(e)) => return Err(map_pdf_inspector_error(e)),
            Err(_) => {
                return Err(ExtractionError::Internal(
                    "PDF text extraction panicked".into(),
                ));
            }
        };

        // Group position-tagged items by their 0-indexed page.
        let mut by_page: HashMap<usize, Vec<String>> = HashMap::new();
        for item in items {
            let page = item.page as usize - 1;
            by_page.entry(page).or_default().push(item.text);
        }

        let mut out = Vec::with_capacity(pages.len());
        for &p in pages {
            // Joining the item texts with a space approximates the page's text
            // layer. Layout/reading-order reflow (paragraphs, columns) is not
            // reproduced exactly — see the module note in the final report.
            let text = by_page.get(&p).map(|v| v.join(" ")).unwrap_or_default();
            out.push(ExtractedPage::local(p, text));
        }
        Ok(out)
    }
}

/// Build an `Arc<dyn PdfTextSource>` for wiring/composition.
pub fn make_pdf_text_source() -> Arc<dyn PdfTextSource> {
    Arc::new(PdfTextSourceAdapter::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors() {
        let a = PdfTextSourceAdapter::new();
        let b = make_pdf_text_source();
        let _ = (a, b);
    }

    #[test]
    fn empty_pages_returns_empty() {
        let a = PdfTextSourceAdapter::new();
        let r = a.extract_pages(&Source::Bytes(vec![1, 2, 3]), &[]).unwrap();
        assert!(r.is_empty());
    }
}
