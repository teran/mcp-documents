//! Domain layer: entities, value objects, ports (traits) and domain errors.
//!
//! This layer must remain **library-agnostic** — no `rmcp`, `anydoc`,
//! `pdf-render` or `tesseract-rs` imports. All external concerns are reached
//! through the ports defined in [`crate::domain::ports`].

pub mod document;
pub mod error;
pub mod meta;
pub mod ports;
pub mod ranges;
pub mod result;

pub use document::{Classification, DocumentKind, PageClassification, PdfType};
pub use error::ExtractionError;
pub use meta::{CsvInfo, PdfInfo, PresentationInfo, SheetInfo, SpreadsheetInfo, TextDocumentInfo};
pub use ports::{
    CsvReader, DocumentExtractor, OcrProvider, PdfPageRenderer, PdfTextSource, PresentationReader,
    SpreadsheetReader, TextDocumentReader,
};
pub use result::{ExtractedPage, ExtractionResult, PageBitmap, PageSource, Source};
