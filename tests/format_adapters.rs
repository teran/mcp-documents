//! Integration tests for the per-format adapters (docx/xlsx/csv/pptx) against
//! committed fixtures and generated in-memory files.

use std::io::Write;

use mcp_documents::domain::document::DocumentKind;
use mcp_documents::domain::ports::{
    CsvReader, DocumentExtractor, PresentationReader, SpreadsheetReader, TextDocumentReader,
};
use mcp_documents::domain::result::Source;
use mcp_documents::infrastructure::{
    AnyDocExtractor, AnyDocPresentationReader, AnyDocTextReader, CalamineSpreadsheetReader,
    CsvStreamReader,
};

fn fixture(name: &str) -> Source {
    Source::Path(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
}

// ---------------------------------------------------------------------
// DOCX — word/char counting + extraction
// ---------------------------------------------------------------------

#[test]
fn docx_info_counts_words_and_chars() {
    let reader = AnyDocTextReader::new();
    let info = reader.text_info(&fixture("simple_text.docx")).unwrap();
    // "Hello docx from mcp-documents" (anydoc output may include a trailing
    // newline, so allow 29 or 30 characters).
    assert_eq!(info.word_count, 4);
    assert!(info.char_count == 29 || info.char_count == 30);
}

#[test]
fn docx_extract_returns_markdown() {
    let reader = AnyDocTextReader::new();
    let md = reader.extract_text(&fixture("simple_text.docx")).unwrap();
    assert!(md.contains("Hello docx from mcp-documents"));
}

#[test]
fn classifies_docx_fixture_as_docx() {
    let extractor = AnyDocExtractor::new();
    let c = extractor.classify(&fixture("simple_text.docx")).unwrap();
    assert_eq!(c.kind, DocumentKind::Docx);
}

// ---------------------------------------------------------------------
// XLSX — sheet listing + sheet extraction
// ---------------------------------------------------------------------

#[test]
fn xlsx_info_lists_sheets() {
    let reader = CalamineSpreadsheetReader::new();
    let info = reader
        .spreadsheet_info(&fixture("simple_text.xlsx"))
        .unwrap();
    assert!(!info.sheets.is_empty());
    let sheet = &info.sheets[0];
    assert_eq!(sheet.name, "Sheet1");
    assert!(sheet.rows.unwrap_or(0) >= 1);
}

#[test]
fn xlsx_extract_sheet_returns_markdown_table() {
    let reader = CalamineSpreadsheetReader::new();
    let md = reader
        .extract_sheet(&fixture("simple_text.xlsx"), None, None)
        .unwrap();
    assert!(md.contains("Hello xlsx from mcp-documents"));
}

#[test]
fn classifies_xlsx_fixture_as_xlsx() {
    let extractor = AnyDocExtractor::new();
    let c = extractor.classify(&fixture("simple_text.xlsx")).unwrap();
    assert_eq!(c.kind, DocumentKind::Xlsx);
}

// ---------------------------------------------------------------------
// CSV — streaming info + row-range extraction
// ---------------------------------------------------------------------

#[test]
fn csv_info_and_row_range() {
    let dir = std::env::temp_dir().join(format!("mcp-int-csv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("data.csv");
    std::fs::write(&path, "a,b,c\n1,2,3\n4,5,6\n7,8,9\n").unwrap();

    let reader = CsvStreamReader::new();
    let info = reader.csv_info(&Source::Path(path.clone())).unwrap();
    assert_eq!(info.row_count, 3);
    assert_eq!(info.column_count, 3);
    assert_eq!(info.header, vec!["a", "b", "c"]);

    let md = reader
        .extract_csv(&Source::Path(path.clone()), Some(1..2))
        .unwrap();
    assert!(md.contains("| 4 | 5 | 6 |"));
    assert!(!md.contains("| 1 | 2 | 3 |"));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------
// PPTX — slide count + slide-range extraction (in-memory OOXML zip)
// ---------------------------------------------------------------------

fn make_pptx_bytes(slides: &[&str]) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let opts = zip::write::SimpleFileOptions::default();
    // Minimal presentation part so content-based detection identifies it as
    // PPTX (anydoc's conventional-main-part fallback).
    zip.start_file("ppt/presentation.xml", opts).unwrap();
    zip.write_all(b"<p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"></p:presentation>").unwrap();
    for (i, text) in slides.iter().enumerate() {
        let name = format!("ppt/slides/slide{}.xml", i + 1);
        let xml = format!(
            "<p:sld><p:cSld><p:sp><a:t>{}</a:t></p:sp></p:cSld></p:sld>",
            text
        );
        zip.start_file(name, opts).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn pptx_info_counts_slides() {
    let bytes = make_pptx_bytes(&["One", "Two", "Three"]);
    let reader = AnyDocPresentationReader::new(None);
    let info = reader.presentation_info(&Source::Bytes(bytes)).unwrap();
    assert_eq!(info.slide_count, 3);
}

#[test]
fn pptx_extract_slide_range() {
    let bytes = make_pptx_bytes(&["Slide one", "Slide two", "Slide three"]);
    let reader = AnyDocPresentationReader::new(None);
    let md = reader
        .extract_presentation(&Source::Bytes(bytes.clone()), Some(0..2))
        .unwrap();
    assert!(md.contains("Slide one"));
    assert!(md.contains("Slide two"));
    assert!(!md.contains("Slide three"));

    let all = reader
        .extract_presentation(&Source::Bytes(bytes), None)
        .unwrap();
    assert!(all.contains("Slide three"));
}

#[test]
fn classify_pptx_magic_as_pptx() {
    let bytes = make_pptx_bytes(&["X"]);
    let extractor = AnyDocExtractor::new();
    let c = extractor.classify(&Source::Bytes(bytes)).unwrap();
    assert_eq!(c.kind, DocumentKind::Pptx);
}
