//! Entities and value objects describing documents.

/// The family of document a file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocumentKind {
    /// Portable Document Format.
    Pdf,
    /// Word 97–2003 (binary `.doc`).
    Doc,
    /// WordprocessingML (`.docx`, `.docm`).
    Docx,
    /// Excel 97–2003 (binary `.xls`).
    Xls,
    /// SpreadsheetML (`.xlsx`, `.xlsm`, `.xlsb`).
    Xlsx,
    /// PowerPoint 97–2003 (binary `.ppt`, `.pps`, `.pot`).
    Ppt,
    /// PresentationML (`.pptx`, `.pptm`, `.ppsx`, `.ppsm`).
    Pptx,
    /// OpenDocument Text (`.odt`).
    Odt,
    /// OpenDocument Spreadsheet (`.ods`).
    Ods,
    /// OpenDocument Presentation (`.odp`).
    Odp,
    /// Rich Text Format (`.rtf`).
    Rtf,
    /// EPUB 2 and 3 (`.epub`).
    Epub,
    /// Delimiter-separated text (`.csv`).
    Csv,
    /// A document whose kind could not be determined.
    Unknown,
}

impl DocumentKind {
    /// Human-readable name for the kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            DocumentKind::Pdf => "pdf",
            DocumentKind::Doc => "doc",
            DocumentKind::Docx => "docx",
            DocumentKind::Xls => "xls",
            DocumentKind::Xlsx => "xlsx",
            DocumentKind::Ppt => "ppt",
            DocumentKind::Pptx => "pptx",
            DocumentKind::Odt => "odt",
            DocumentKind::Ods => "ods",
            DocumentKind::Odp => "odp",
            DocumentKind::Rtf => "rtf",
            DocumentKind::Epub => "epub",
            DocumentKind::Csv => "csv",
            DocumentKind::Unknown => "unknown",
        }
    }
}

/// High-level classification of a PDF based on whether pages contain a text
/// layer or are image-only / scanned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfType {
    /// Every page has a usable text layer — no OCR needed.
    TextBased,
    /// Every page is a scan / image with no text layer — OCR needed for all.
    Scanned,
    /// Pages carry images (charts/photos) but also a text layer.
    ImageBased,
    /// A mix of text-based and scanned/image-only pages.
    Mixed,
}

impl PdfType {
    /// Machine-readable name for the type.
    pub fn as_str(&self) -> &'static str {
        match self {
            PdfType::TextBased => "TextBased",
            PdfType::Scanned => "Scanned",
            PdfType::ImageBased => "ImageBased",
            PdfType::Mixed => "Mixed",
        }
    }
}

/// Per-page classification result.
#[derive(Debug, Clone, PartialEq)]
pub struct PageClassification {
    /// Zero-based index of the page.
    pub page_index: usize,
    /// Whether this page requires OCR to produce text.
    pub needs_ocr: bool,
    /// Confidence (0.0..=1.0) that the page is classified correctly.
    pub confidence: f32,
}

impl PageClassification {
    /// Create a new page classification.
    pub fn new(page_index: usize, needs_ocr: bool, confidence: f32) -> Self {
        Self {
            page_index,
            needs_ocr,
            confidence,
        }
    }
}

/// Result of classifying a document.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    /// The detected document kind.
    pub kind: DocumentKind,
    /// PDF-specific type (only present for PDFs).
    pub pdf_type: Option<PdfType>,
    /// Per-page classification for PDFs.
    pub pages_needing_ocr: Vec<PageClassification>,
}

impl Classification {
    /// Convenience accessor for the [`PdfType`] of a PDF, or `None`.
    pub fn pdf_type(&self) -> Option<PdfType> {
        self.pdf_type
    }

    /// Number of pages that require OCR.
    pub fn ocr_page_count(&self) -> usize {
        self.pages_needing_ocr
            .iter()
            .filter(|p| p.needs_ocr)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_kind_as_str() {
        assert_eq!(DocumentKind::Pdf.as_str(), "pdf");
        assert_eq!(DocumentKind::Doc.as_str(), "doc");
        assert_eq!(DocumentKind::Docx.as_str(), "docx");
        assert_eq!(DocumentKind::Xls.as_str(), "xls");
        assert_eq!(DocumentKind::Xlsx.as_str(), "xlsx");
        assert_eq!(DocumentKind::Ppt.as_str(), "ppt");
        assert_eq!(DocumentKind::Pptx.as_str(), "pptx");
        assert_eq!(DocumentKind::Odt.as_str(), "odt");
        assert_eq!(DocumentKind::Ods.as_str(), "ods");
        assert_eq!(DocumentKind::Odp.as_str(), "odp");
        assert_eq!(DocumentKind::Rtf.as_str(), "rtf");
        assert_eq!(DocumentKind::Epub.as_str(), "epub");
        assert_eq!(DocumentKind::Csv.as_str(), "csv");
        assert_eq!(DocumentKind::Unknown.as_str(), "unknown");
    }

    #[test]
    fn page_classification_new() {
        let pc = PageClassification::new(2, true, 0.95);
        assert_eq!(pc.page_index, 2);
        assert!(pc.needs_ocr);
        assert_eq!(pc.confidence, 0.95);
    }

    #[test]
    fn classification_helpers() {
        let mut c = Classification {
            kind: DocumentKind::Pdf,
            pdf_type: Some(PdfType::Mixed),
            pages_needing_ocr: vec![
                PageClassification::new(0, false, 0.9),
                PageClassification::new(1, true, 0.95),
                PageClassification::new(2, true, 0.98),
            ],
        };
        assert_eq!(c.pdf_type(), Some(PdfType::Mixed));
        assert_eq!(c.ocr_page_count(), 2);

        c.pdf_type = None;
        assert_eq!(c.pdf_type(), None);
    }

    #[test]
    fn classification_no_ocr_pages() {
        let c = Classification {
            kind: DocumentKind::Pdf,
            pdf_type: Some(PdfType::TextBased),
            pages_needing_ocr: vec![PageClassification::new(0, false, 0.9)],
        };
        assert_eq!(c.ocr_page_count(), 0);
    }

    #[test]
    fn pdf_type_as_str_covers_all_variants() {
        assert_eq!(PdfType::TextBased.as_str(), "TextBased");
        assert_eq!(PdfType::Scanned.as_str(), "Scanned");
        assert_eq!(PdfType::ImageBased.as_str(), "ImageBased");
        assert_eq!(PdfType::Mixed.as_str(), "Mixed");
    }
}
