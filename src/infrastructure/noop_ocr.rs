//! `NoopOcrProvider` — the disabled-OCR adapter used when the `ocr-native`
//! feature is off.
//!
//! It satisfies the `OcrProvider` port so the pipeline compiles and behaves
//! correctly without a real OCR engine: any OCR request fails explicitly with
//! `ExtractionError::OcrUnavailable` rather than silently returning empty
//! content.

use std::sync::Arc;

use crate::domain::error::ExtractionError;
use crate::domain::ports::OcrProvider;
use crate::domain::result::PageBitmap;

/// A stub OCR provider that always reports OCR as unavailable.
#[derive(Debug, Default)]
pub struct NoopOcrProvider {}

impl NoopOcrProvider {
    /// Create the stub provider.
    pub fn new() -> Self {
        Self {}
    }
}

impl OcrProvider for NoopOcrProvider {
    fn ocr_page(&self, _bitmap: &PageBitmap) -> Result<String, ExtractionError> {
        Err(ExtractionError::OcrUnavailable(
            "OCR is not compiled in (build with --features ocr-native)".into(),
        ))
    }
}

/// Build an `Arc<dyn OcrProvider>` for wiring/composition when OCR is off.
pub fn make_ocr() -> Arc<dyn OcrProvider> {
    Arc::new(NoopOcrProvider::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_returns_ocr_unavailable() {
        let p = NoopOcrProvider::new();
        let bmp = PageBitmap::new(1, 1, 1, vec![0]).unwrap();
        let err = p.ocr_page(&bmp).unwrap_err();
        assert!(matches!(err, ExtractionError::OcrUnavailable(_)));
    }
}
