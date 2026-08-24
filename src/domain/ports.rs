//! Ports (traits) that let the application and domain interact with external
//! concerns without depending on any concrete library or framework.

use std::ops::Range;

use crate::domain::document::Classification;
use crate::domain::error::ExtractionError;
use crate::domain::meta::{CsvInfo, PdfInfo, PresentationInfo, SpreadsheetInfo, TextDocumentInfo};
use crate::domain::result::{ExtractedPage, ExtractionResult, PageBitmap, Source};

/// Classifies documents (detect format kind; PDF-specific type + per-page OCR).
///
/// Implemented by an infrastructure adapter (e.g. `AnyDocClassifier`).
pub trait DocumentExtractor: Send + Sync {
    /// Detect the document kind and (for PDFs) the per-page classification.
    fn classify(&self, source: &Source) -> Result<Classification, ExtractionError>;

    /// Extract whole-document Markdown for a **non-PDF** format via `anydoc`.
    /// Used as the fallback path (e.g. legacy `.ppt`, ODF, EPUB).
    fn extract_text(&self, source: &Source) -> Result<ExtractionResult, ExtractionError>;
}

/// Provides per-page text-layer extraction and metadata for PDFs.
///
/// Implemented by an infrastructure adapter backed by `pdf-inspector`, which
/// yields real per-page text plus per-page OCR classification — enabling true
/// Mixed/Scanned/TextBased/ImageBased classification and page-range extraction.
pub trait PdfTextSource: Send + Sync {
    /// PDF metadata: page count, PDF type and per-page OCR needs.
    fn pdf_info(&self, source: &Source) -> Result<PdfInfo, ExtractionError>;

    /// Extract the native text layer for the given 0-indexed pages, returning
    /// one [`ExtractedPage`] per requested page (source `Local`).
    fn extract_pages(
        &self,
        source: &Source,
        pages: &[usize],
    ) -> Result<Vec<ExtractedPage>, ExtractionError>;
}

/// Reads word-processor documents (doc / docx / rtf) — word/char counting.
///
/// These formats have no real page entity, so they expose whole-text word and
/// character counts.
pub trait TextDocumentReader: Send + Sync {
    /// Word and character counts for the whole document.
    fn text_info(&self, source: &Source) -> Result<TextDocumentInfo, ExtractionError>;

    /// The whole document as Markdown (used for `word_range` extraction).
    fn extract_text(&self, source: &Source) -> Result<String, ExtractionError>;
}

/// Reads spreadsheets (xls / xlsx): sheet listing + sheet/row extraction.
///
/// Implemented by an infrastructure adapter backed by `calamine`.
pub trait SpreadsheetReader: Send + Sync {
    /// List the workbook's sheets (with row/col counts where cheap).
    fn spreadsheet_info(&self, source: &Source) -> Result<SpreadsheetInfo, ExtractionError>;

    /// Extract a sheet (by name, or the first sheet when `None`) as a Markdown
    /// table, restricted to the optional 0-based `row_range`.
    fn extract_sheet(
        &self,
        source: &Source,
        sheet: Option<&str>,
        row_range: Option<Range<usize>>,
    ) -> Result<String, ExtractionError>;
}

/// Reads presentations (ppt / pptx): slide count + slide extraction.
///
/// Implemented by an infrastructure adapter that parses PPTX OOXML slide parts
/// directly (so `slide_range` works) and falls back to `anydoc` whole-document
/// extraction for legacy binary `.ppt` (which has no slide range).
pub trait PresentationReader: Send + Sync {
    /// Slide count (1 for legacy `.ppt` — a whole-document unit).
    fn presentation_info(&self, source: &Source) -> Result<PresentationInfo, ExtractionError>;

    /// Extract the requested 0-based `slide_range` as Markdown. For legacy
    /// `.ppt` the range is ignored (whole-document output).
    fn extract_presentation(
        &self,
        source: &Source,
        slide_range: Option<Range<usize>>,
    ) -> Result<String, ExtractionError>;
}

/// Streaming CSV reader (memory < file size).
///
/// Implemented by an infrastructure adapter backed by the `csv` crate.
pub trait CsvReader: Send + Sync {
    /// One streaming pass: total row count, column count and header (O(1) memory).
    fn csv_info(&self, source: &Source) -> Result<CsvInfo, ExtractionError>;

    /// Extract rows in the optional 0-based `row_range` as a Markdown table.
    /// Skips to the start row and reads only the requested range (O(range)
    /// memory).
    fn extract_csv(
        &self,
        source: &Source,
        row_range: Option<Range<usize>>,
    ) -> Result<String, ExtractionError>;
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
