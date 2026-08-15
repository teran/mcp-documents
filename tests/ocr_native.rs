//! Integration tests for the real OCR path (`ocr-native` feature only).
//!
//! These run `TesseractOcrProvider` (embedded eng+rus tessdata) against a
//! bitmap rendered by `PdfRenderer` (pure-Rust `pdf_render`) from a committed
//! fixture. The rendered page is a *synthetic scanned page* — the rasterized
//! image has no text layer, so it genuinely requires OCR to recover the text.
//!
//! Requires the `ocr-native` build (cmake + pre-provisioned `rus.traineddata`).

#![cfg(feature = "ocr-native")]

use mcp_documents::domain::ports::{OcrProvider, PdfPageRenderer};
use mcp_documents::domain::result::Source;
use mcp_documents::infrastructure::{PdfRenderer, TesseractOcrProvider};

fn fixture(name: &str) -> Source {
    Source::Path(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
}

/// OCR a rendered page of the committed text PDF and recover non-empty text.
///
/// The provider is initialized with `eng+rus` (Cyrillic-capable); the rendered
/// page says "Hello, world from mcp-documents".
#[test]
fn ocr_recovers_text_from_rendered_page() {
    let provider = TesseractOcrProvider::new().expect("OCR engine should init (eng+rus)");
    let renderer = PdfRenderer::new();

    let bytes = fixture("simple_text.pdf").read_bytes().unwrap();
    let bitmap = renderer.render_page(&bytes, 0, 300).expect("render page");
    let text = provider.ocr_page(&bitmap).expect("OCR should succeed");

    assert!(!text.trim().is_empty(), "OCR returned empty text");
    let lower = text.to_lowercase();
    assert!(
        lower.contains("mcp")
            || lower.contains("hello")
            || lower.contains("world")
            || lower.contains("document"),
        "OCR text did not match the fixture content: {text:?}"
    );
}

/// The provider can be constructed explicitly with the Cyrillic-capable
/// language spec, confirming the eng+rus init path works end-to-end.
#[test]
fn provider_inits_with_eng_rus() {
    let provider = TesseractOcrProvider::new_with_languages("eng+rus")
        .expect("eng+rus init should succeed on the OCR build");
    let renderer = PdfRenderer::new();
    let bytes = fixture("simple_text.pdf").read_bytes().unwrap();
    let bitmap = renderer.render_page(&bytes, 0, 300).expect("render page");
    let text = provider.ocr_page(&bitmap).expect("OCR should succeed");
    assert!(!text.trim().is_empty());
}
