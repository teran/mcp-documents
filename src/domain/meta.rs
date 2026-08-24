//! Format-specific metadata value objects returned by `*_info` tools.

use crate::domain::document::{PageClassification, PdfType};

/// Metadata for a PDF: page count, classification type and per-page OCR needs.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfInfo {
    /// Total number of pages.
    pub page_count: usize,
    /// The detected PDF type (TextBased / Scanned / ImageBased / Mixed).
    pub pdf_type: PdfType,
    /// Per-page classification (0-indexed), marking which pages need OCR.
    pub pages_needing_ocr: Vec<PageClassification>,
}

/// Metadata for word-processor documents (doc / docx / rtf).
///
/// These formats have **no real page entity** — pagination is computed by the
/// word processor at render time — so they expose word/char counts instead of
/// pages. A "word" is whitespace-delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextDocumentInfo {
    /// Number of whitespace-delimited words in the extracted text.
    pub word_count: usize,
    /// Number of characters in the extracted text.
    pub char_count: usize,
}

/// Information about a single worksheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetInfo {
    /// The sheet name.
    pub name: String,
    /// Number of rows (data rows in the used range), if cheaply known.
    pub rows: Option<usize>,
    /// Number of columns (data columns in the used range), if cheaply known.
    pub cols: Option<usize>,
}

/// Metadata for spreadsheets (xls / xlsx): the list of sheets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadsheetInfo {
    /// The sheets in workbook order.
    pub sheets: Vec<SheetInfo>,
}

/// Metadata for presentations (ppt / pptx): the slide count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationInfo {
    /// Number of slides. For legacy binary `.ppt` (routed through `anydoc` as a
    /// whole-document unit) this reports `1` because slides are not separately
    /// addressable — see the presentation reader docs.
    pub slide_count: usize,
}

/// Metadata for a CSV file, gathered in a single streaming pass (O(1) memory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvInfo {
    /// Total number of data rows (excluding the header row).
    pub row_count: usize,
    /// Number of columns (width of the header/first row).
    pub column_count: usize,
    /// The header row (first record). Empty when the file is empty.
    pub header: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_info_constructs() {
        let info = PdfInfo {
            page_count: 3,
            pdf_type: PdfType::Mixed,
            pages_needing_ocr: vec![PageClassification::new(1, true, 1.0)],
        };
        assert_eq!(info.page_count, 3);
        assert_eq!(info.pdf_type, PdfType::Mixed);
        assert_eq!(info.pages_needing_ocr[0].page_index, 1);
    }

    #[test]
    fn text_document_info_counts() {
        let info = TextDocumentInfo {
            word_count: 12,
            char_count: 64,
        };
        assert_eq!(info.word_count, 12);
        assert_eq!(info.char_count, 64);
    }

    #[test]
    fn spreadsheet_info_lists_sheets() {
        let info = SpreadsheetInfo {
            sheets: vec![
                SheetInfo {
                    name: "Sheet1".into(),
                    rows: Some(3),
                    cols: Some(2),
                },
                SheetInfo {
                    name: "Sheet2".into(),
                    rows: None,
                    cols: None,
                },
            ],
        };
        assert_eq!(info.sheets.len(), 2);
        assert_eq!(info.sheets[0].name, "Sheet1");
    }

    #[test]
    fn presentation_info_slide_count() {
        let info = PresentationInfo { slide_count: 5 };
        assert_eq!(info.slide_count, 5);
    }

    #[test]
    fn csv_info_header_and_counts() {
        let info = CsvInfo {
            row_count: 10,
            column_count: 3,
            header: vec!["a".into(), "b".into(), "c".into()],
        };
        assert_eq!(info.row_count, 10);
        assert_eq!(info.column_count, 3);
        assert_eq!(info.header, vec!["a", "b", "c"]);
    }
}
