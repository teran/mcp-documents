//! Application layer: use cases that orchestrate business rules.
//!
//! Use cases depend **only** on the domain ports — never on `anydoc`, `rmcp`,
//! `pdf-render` or `tesseract-rs`.

pub mod classify_document;
pub mod csv;
pub mod pdf;
pub mod presentation;
pub mod spreadsheet;
pub mod text_doc;

pub use classify_document::ClassifyDocumentUseCase;
pub use csv::CsvUseCase;
pub use pdf::{OCR_DPI, PdfUseCase};
pub use presentation::PresentationUseCase;
pub use spreadsheet::SpreadsheetUseCase;
pub use text_doc::TextDocUseCase;
