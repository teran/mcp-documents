//! Infrastructure layer: concrete adapters that implement the domain ports.
//!
//! This layer may import `anydoc`, `pdf-render`, and (behind the `ocr-native`
//! feature) `tesseract-rs`. It is responsible for mapping library errors onto
//! domain [`ExtractionError`](crate::domain::error::ExtractionError)s via
//! [`error_map`], and for isolating conversion/parse calls with panic
//! recovery so a malformed document never aborts the process.

pub mod anydoc_extractor;
pub mod error_map;
pub mod file_reader;
pub mod pdf_renderer;

#[cfg(not(feature = "ocr-native"))]
pub mod noop_ocr;
#[cfg(feature = "ocr-native")]
pub mod tesseract_ocr;

pub use anydoc_extractor::AnyDocExtractor;
pub use file_reader::{read_file, read_source_bytes};
pub use pdf_renderer::PdfRenderer;

#[cfg(not(feature = "ocr-native"))]
pub use noop_ocr::make_ocr;
#[cfg(feature = "ocr-native")]
pub use tesseract_ocr::make_ocr;

#[cfg(not(feature = "ocr-native"))]
pub use noop_ocr::NoopOcrProvider;
#[cfg(feature = "ocr-native")]
pub use tesseract_ocr::TesseractOcrProvider;
