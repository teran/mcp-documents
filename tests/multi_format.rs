//! Integration tests for multi-format (MS Office) extraction via `anydoc`.
//!
//! These run with **default features** (pure Rust, no OCR) and exercise the
//! real `anydoc` paths on small committed `.docx` / `.xlsx` fixtures.

use mcp_documents::domain::document::DocumentKind;
use mcp_documents::domain::ports::DocumentExtractor;
use mcp_documents::domain::result::Source;
use mcp_documents::infrastructure::AnyDocExtractor;

fn fixture(name: &str) -> Source {
    Source::Path(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
}

#[test]
fn extracts_docx_fixture_to_markdown() {
    let extractor = AnyDocExtractor::new();
    let result = extractor
        .extract_text(&fixture("simple_text.docx"))
        .unwrap();
    assert_eq!(result.kind, DocumentKind::Docx);
    // Non-PDF kinds carry no PDF type.
    assert_eq!(result.pdf_type, None);
    assert!(result.markdown.contains("Hello docx from mcp-documents"));
}

#[test]
fn classifies_docx_fixture_kind_without_ocr() {
    let extractor = AnyDocExtractor::new();
    let classification = extractor.classify(&fixture("simple_text.docx")).unwrap();
    assert_eq!(classification.kind, DocumentKind::Docx);
    assert_eq!(classification.pdf_type, None);
    assert!(classification.pages_needing_ocr.is_empty());
}

#[test]
fn extracts_xlsx_fixture_to_markdown() {
    let extractor = AnyDocExtractor::new();
    let result = extractor
        .extract_text(&fixture("simple_text.xlsx"))
        .unwrap();
    assert_eq!(result.kind, DocumentKind::Xlsx);
    assert_eq!(result.pdf_type, None);
    assert!(result.markdown.contains("Hello xlsx from mcp-documents"));
}

#[test]
fn classifies_xlsx_fixture_kind_without_ocr() {
    let extractor = AnyDocExtractor::new();
    let classification = extractor.classify(&fixture("simple_text.xlsx")).unwrap();
    assert_eq!(classification.kind, DocumentKind::Xlsx);
    assert_eq!(classification.pdf_type, None);
    assert!(classification.pages_needing_ocr.is_empty());
}

#[test]
fn unknown_bytes_classify_as_unsupported() {
    let extractor = AnyDocExtractor::new();
    let err = extractor
        .classify(&Source::Bytes(b"not any document".to_vec()))
        .unwrap_err();
    assert!(matches!(
        err,
        mcp_documents::domain::error::ExtractionError::Unsupported(DocumentKind::Unknown)
    ));
}
