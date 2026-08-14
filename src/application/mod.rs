//! Application layer: use cases that orchestrate business rules.
//!
//! Use cases depend **only** on the domain ports — never on `anydoc`, `rmcp`,
//! `pdf-render` or `tesseract-rs`.

pub mod classify_document;
pub mod extract_document;

pub use classify_document::ClassifyDocumentUseCase;
pub use extract_document::ExtractDocumentUseCase;
