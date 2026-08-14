//! Ports (traits) that let the application and domain interact with external
//! concerns without depending on any concrete library or framework.

use crate::domain::document::Classification;
use crate::domain::error::ExtractionError;
use crate::domain::result::{ExtractionResult, PageBitmap, Source};

/// Extracts text/Markdown from documents and classifies them.
///
/// Implemented by infrastructure adapters (e.g. `AnyDocExtractor`). The
/// application layer depends only on this port, never on `anydoc` directly.
pub trait DocumentExtractor: Send + Sync {
    /// Detect the document kind and PDF classification from content.
    fn classify(&self, source: &Source) -> Result<Classification, ExtractionError>;

    /// Extract Markdown/text from a document (the text-based path).
    fn extract_text(&self, source: &Source) -> Result<ExtractionResult, ExtractionError>;
}

/// Performs OCR on a single page bitmap, returning extracted text.
///
/// Implemented by infrastructure adapters (e.g. `TesseractOcrProvider`, or a
/// `NoopOcrProvider` when the OCR feature is disabled). Consumes **rendered
/// page bitmaps only** — never a PDF directly. The bitmap carries its real
/// pixel dimensions so OCR geometry (width/height/bytes-per-line) is correct.
pub trait OcrProvider: Send + Sync {
    /// OCR a single page image, returning extracted text.
    fn ocr_page(&self, bitmap: &PageBitmap) -> Result<String, ExtractionError>;
}

/// Renders a single PDF page to a bitmap for OCR consumption.
///
/// Keeps the PDF → image rasterizer swappable behind a port (the chosen
/// implementation is the pure-Rust `pdf_render` crate). Returns the bitmap
/// together with its real rendered dimensions.
pub trait PdfPageRenderer: Send + Sync {
    /// Render a single PDF page to a bitmap (grayscale) for OCR consumption.
    fn render_page(
        &self,
        pdf_bytes: &[u8],
        page_index: usize,
        dpi: u32,
    ) -> Result<PageBitmap, ExtractionError>;
}
