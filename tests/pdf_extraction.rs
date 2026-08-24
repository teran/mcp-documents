//! Integration tests for infrastructure adapters against committed fixtures.
//!
//! These run with **default features** (no OCR) and exercise the real
//! `pdf-inspector` / `pdf-render` / `anydoc` paths on a small committed text
//! PDF.

use mcp_documents::domain::document::PdfType;
use mcp_documents::domain::ports::{DocumentExtractor, PdfPageRenderer, PdfTextSource};
use mcp_documents::domain::result::Source;
use mcp_documents::infrastructure::{AnyDocExtractor, PdfRenderer, PdfTextSourceAdapter};

fn fixture(name: &str) -> Source {
    Source::Path(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
}

#[test]
fn anydoc_extracts_text_from_fixture_pdf() {
    let extractor = AnyDocExtractor::new();
    let result = extractor.extract_text(&fixture("simple_text.pdf")).unwrap();
    assert_eq!(result.kind.as_str(), "pdf");
    // The fixture contains this literal text.
    assert!(result.markdown.contains("Hello, world from mcp-documents"));
}

#[test]
fn anydoc_classifies_fixture_pdf_as_text_based() {
    let extractor = AnyDocExtractor::new();
    let classification = extractor.classify(&fixture("simple_text.pdf")).unwrap();
    assert_eq!(classification.kind.as_str(), "pdf");
    assert_eq!(classification.pdf_type, Some(PdfType::TextBased));
    assert!(classification.pages_needing_ocr.is_empty());
}

#[test]
fn pdf_text_source_reports_info_and_extracts_pages() {
    let source = fixture("simple_text.pdf");
    let adapter = PdfTextSourceAdapter::new();
    let info = adapter.pdf_info(&source).unwrap();
    assert_eq!(info.pdf_type, PdfType::TextBased);
    assert_eq!(info.page_count, 1);
    assert!(info.pages_needing_ocr.is_empty());

    let pages = adapter.extract_pages(&source, &[0]).unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].page_index, 0);
    assert!(pages[0].text.to_lowercase().contains("mcp-documents"));
}

#[test]
fn pdf_renderer_renders_fixture_page() {
    let renderer = PdfRenderer::new();
    let bytes = fixture("simple_text.pdf").read_bytes().unwrap();
    // Render page 0 at 72 DPI: must produce a non-empty grayscale bitmap with
    // real (non-zero) dimensions.
    let bitmap = renderer.render_page(&bytes, 0, 72).unwrap();
    assert!(bitmap.width > 0 && bitmap.height > 0);
    assert_eq!(bitmap.bytes_per_pixel, 1);
    assert!(!bitmap.data.is_empty());
    assert_eq!(
        bitmap.data.len(),
        bitmap.width as usize * bitmap.height as usize
    );
}

#[test]
fn pdf_renderer_rejects_bad_pdf() {
    let renderer = PdfRenderer::new();
    let err = renderer
        .render_page(b"this is not a pdf", 0, 72)
        .unwrap_err();
    // Mapped to a domain error (Malformed or Internal), never a panic.
    let _ = err;
}
