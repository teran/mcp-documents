//! Extraction results and the input `Source` value object.

use std::path::PathBuf;

use crate::domain::document::{DocumentKind, PdfType};
use crate::domain::error::ExtractionError;

/// The origin of a page's extracted text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSource {
    /// Text came from the document's native text layer (local extraction).
    Local,
    /// Text came from OCR of a rendered page bitmap.
    Ocr,
}

/// A single page's extracted text.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedPage {
    /// Zero-based page index.
    pub page_index: usize,
    /// Where the text came from.
    pub source: PageSource,
    /// The extracted text for this page.
    pub text: String,
}

impl ExtractedPage {
    /// Create a local page entry.
    pub fn local(page_index: usize, text: impl Into<String>) -> Self {
        Self {
            page_index,
            source: PageSource::Local,
            text: text.into(),
        }
    }

    /// Create an OCR page entry.
    pub fn ocr(page_index: usize, text: impl Into<String>) -> Self {
        Self {
            page_index,
            source: PageSource::Ocr,
            text: text.into(),
        }
    }
}

/// The full result of extracting a document.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionResult {
    /// The document kind.
    pub kind: DocumentKind,
    /// PDF-specific type (when the document is a PDF).
    pub pdf_type: Option<PdfType>,
    /// The complete Markdown text.
    pub markdown: String,
    /// Per-page extracted text.
    pub pages: Vec<ExtractedPage>,
}

impl ExtractionResult {
    /// Build a result with a Markdown body and no per-page breakdown.
    pub fn markdown_only(kind: DocumentKind, pdf_type: Option<PdfType>, markdown: String) -> Self {
        Self {
            kind,
            pdf_type,
            markdown,
            pages: Vec::new(),
        }
    }

    /// Build a result from an ordered list of pages, deriving the Markdown
    /// body by joining the page texts in order.
    pub fn from_pages(
        kind: DocumentKind,
        pdf_type: Option<PdfType>,
        pages: Vec<ExtractedPage>,
    ) -> Self {
        let markdown = merge_pages(&pages);
        Self {
            kind,
            pdf_type,
            markdown,
            pages,
        }
    }
}

/// Join page texts into a single Markdown body, preserving page order.
pub fn merge_pages(pages: &[ExtractedPage]) -> String {
    let mut ordered = pages.to_vec();
    ordered.sort_by_key(|p| p.page_index);
    ordered
        .into_iter()
        .map(|p| p.text)
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A rasterized page bitmap produced by a PDF renderer for OCR consumption.
///
/// Carries the real pixel dimensions so an OCR provider can call
/// `set_image(data, width, height, bytes_per_pixel, bytes_per_row)` with the
/// correct geometry — rather than guessing a square page (which misreads any
/// non-square scan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBitmap {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Number of bytes per pixel (1 = grayscale, 3 = RGB, 4 = RGBA).
    pub bytes_per_pixel: u8,
    /// Pixel data, tightly packed, row-major (top-to-bottom).
    pub data: Vec<u8>,
}

impl PageBitmap {
    /// Create a bitmap, validating that the data length matches the geometry.
    pub fn new(
        width: u32,
        height: u32,
        bytes_per_pixel: u8,
        data: Vec<u8>,
    ) -> Result<Self, ExtractionError> {
        let expected = width as usize * height as usize * bytes_per_pixel as usize;
        if data.len() != expected {
            return Err(ExtractionError::Internal(format!(
                "bitmap buffer size mismatch: expected {expected} bytes for {width}x{height}x{bytes_per_pixel}, got {}",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            bytes_per_pixel,
            data,
        })
    }

    /// Bytes per row (stride) for the tight-packed layout.
    pub fn bytes_per_row(&self) -> usize {
        self.width as usize * self.bytes_per_pixel as usize
    }
}

/// Input document source. Owns its data so it can be moved into
/// `spawn_blocking` / `'static` closures without lifetime fights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A path to a file on disk.
    Path(PathBuf),
    /// In-memory document bytes.
    Bytes(Vec<u8>),
}

impl Source {
    /// The path, if this source is a [`Source::Path`].
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Source::Path(p) => Some(p.as_path()),
            Source::Bytes(_) => None,
        }
    }

    /// A short display label for diagnostics.
    pub fn label(&self) -> String {
        match self {
            Source::Path(p) => p.display().to_string(),
            Source::Bytes(b) => format!("<{} bytes>", b.len()),
        }
    }

    /// Resolve the source to in-memory bytes, reading from disk if it is a
    /// path. Used when the pipeline needs the raw bytes (e.g. to render pages
    /// for OCR).
    pub fn read_bytes(&self) -> Result<Vec<u8>, crate::domain::error::ExtractionError> {
        match self {
            Source::Path(p) => std::fs::read(p).map_err(|e| {
                crate::domain::error::ExtractionError::Io(format!(
                    "failed to read {}: {e}",
                    p.display()
                ))
            }),
            Source::Bytes(b) => Ok(b.clone()),
        }
    }
}

impl From<PathBuf> for Source {
    fn from(p: PathBuf) -> Self {
        Source::Path(p)
    }
}

impl From<Vec<u8>> for Source {
    fn from(b: Vec<u8>) -> Self {
        Source::Bytes(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_source_variants() {
        assert_eq!(PageSource::Local, PageSource::Local);
        assert_ne!(PageSource::Local, PageSource::Ocr);
    }

    #[test]
    fn extracted_page_constructors() {
        let local = ExtractedPage::local(0, "hello");
        assert_eq!(local.source, PageSource::Local);
        assert_eq!(local.text, "hello");
        let ocr = ExtractedPage::ocr(1, "world");
        assert_eq!(ocr.source, PageSource::Ocr);
        assert_eq!(ocr.text, "world");
    }

    #[test]
    fn merge_pages_preserves_order_and_skips_empty() {
        let pages = vec![
            ExtractedPage::ocr(2, "page three"),
            ExtractedPage::local(0, "page one"),
            ExtractedPage::ocr(1, ""),
            ExtractedPage::local(3, "page four"),
        ];
        assert_eq!(merge_pages(&pages), "page one\n\npage three\n\npage four");
    }

    #[test]
    fn extraction_result_markdown_only() {
        let r = ExtractionResult::markdown_only(DocumentKind::Pdf, None, "body".into());
        assert_eq!(r.markdown, "body");
        assert!(r.pages.is_empty());
    }

    #[test]
    fn extraction_result_from_pages() {
        let r = ExtractionResult::from_pages(
            DocumentKind::Pdf,
            Some(PdfType::Mixed),
            vec![ExtractedPage::ocr(0, "a"), ExtractedPage::local(1, "b")],
        );
        assert_eq!(r.markdown, "a\n\nb");
        assert_eq!(r.pages.len(), 2);
    }

    #[test]
    fn page_bitmap_valid_geometry() {
        let bmp = PageBitmap::new(640, 480, 1, vec![0u8; 640 * 480]).unwrap();
        assert_eq!(bmp.width, 640);
        assert_eq!(bmp.height, 480);
        assert_eq!(bmp.bytes_per_pixel, 1);
        assert_eq!(bmp.bytes_per_row(), 640);
    }

    #[test]
    fn page_bitmap_rgba_geometry() {
        let bmp = PageBitmap::new(10, 20, 4, vec![0u8; 10 * 20 * 4]).unwrap();
        assert_eq!(bmp.bytes_per_row(), 40);
    }

    #[test]
    fn page_bitmap_mismatched_buffer_is_internal_error() {
        let err = PageBitmap::new(640, 480, 1, vec![0u8; 100]).unwrap_err();
        assert!(matches!(err, ExtractionError::Internal(_)));
    }

    #[test]
    fn source_variants_and_conversions() {
        let s = Source::from(PathBuf::from("/tmp/x.pdf"));
        assert_eq!(s.path(), Some(std::path::Path::new("/tmp/x.pdf")));
        assert_eq!(s.label(), "/tmp/x.pdf");

        let b = Source::from(vec![1, 2, 3]);
        assert!(b.path().is_none());
        assert_eq!(b.label(), "<3 bytes>");
    }

    #[test]
    fn source_read_bytes() {
        let b = Source::Bytes(vec![1, 2, 3]);
        assert_eq!(b.read_bytes().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn source_read_bytes_missing_path_is_io_error() {
        let s = Source::Path(PathBuf::from("/definitely/missing/does-not-exist.bin"));
        let err = s.read_bytes().unwrap_err();
        assert!(matches!(err, ExtractionError::Io(_)));
    }
}
