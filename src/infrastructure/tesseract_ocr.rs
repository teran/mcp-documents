//! `TesseractOcrProvider` — implements [`OcrProvider`] using `tesseract-rs`
//! with embedded tessdata (English + Russian).
//!
//! Only compiled when the `ocr-native` feature is enabled (requires a C++
//! compiler + CMake + internet at build time, and the
//! `TESSERACT_EMBED_LANGUAGES=eng,rus` env var with pre-provisioned
//! `rus.traineddata`). See SPEC §4.3.
//!
//! # Init gotcha
//! `api.init_embedded("eng+rus")` does **not** work — the embedded tessdata map
//! is keyed per single language. Instead we materialize the embedded bytes into
//! a temp dir (`get_embedded_tessdata` / `embedded_languages`) and call
//! `api.init(temp_dir, "eng+rus")`. The bytes come from the embedded binary,
//! not disk/network, preserving the zero-runtime-dependency property.

use std::sync::Arc;

use crate::domain::error::ExtractionError;
use crate::domain::ports::OcrProvider;
use crate::domain::result::PageBitmap;

/// A Tesseract-backed OCR provider consuming rendered page bitmaps.
///
/// `new` performs the (expensive) engine init by materializing embedded
/// tessdata to a temp dir. Keep a single instance and clone per worker via
/// [`TesseractAPI::try_clone`] for parallel OCR.
pub struct TesseractOcrProvider {
    api: tesseract_rs::TesseractAPI,
    /// Kept alive for the lifetime of the provider (contains the materialized
    /// `.traineddata` files).
    _temp_dir: std::path::PathBuf,
}

impl TesseractOcrProvider {
    /// Initialize the engine once (embedded eng+rus tessdata materialized to a
    /// temp dir). Returns `OcrUnavailable` if the engine cannot be initialized.
    pub fn new() -> Result<Self, ExtractionError> {
        Self::new_with_languages("eng+rus")
    }

    /// Initialize with an explicit language spec.
    pub fn new_with_languages(langs: &str) -> Result<Self, ExtractionError> {
        let temp_dir =
            std::env::temp_dir().join(format!("mcp-documents-tessdata-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| ExtractionError::Io(format!("create tessdata dir: {e}")))?;

        // Materialize each embedded language's traineddata into the temp dir.
        for lang in tesseract_rs::embedded_languages() {
            let Some(bytes) = tesseract_rs::get_embedded_tessdata(lang) else {
                continue;
            };
            let path = temp_dir.join(format!("{lang}.traineddata"));
            std::fs::write(&path, bytes)
                .map_err(|e| ExtractionError::Io(format!("write {lang}.traineddata: {e}")))?;
        }

        let api = tesseract_rs::TesseractAPI::new();
        api.init(&temp_dir, langs)
            .map_err(|e| ExtractionError::OcrUnavailable(format!("tesseract init: {e:?}")))?;

        Ok(Self {
            api,
            _temp_dir: temp_dir,
        })
    }
}

impl OcrProvider for TesseractOcrProvider {
    fn ocr_page(&self, bitmap: &PageBitmap) -> Result<String, ExtractionError> {
        // Use the bitmap's real geometry (width/height/bytes-per-line), so
        // non-square scans (e.g. portrait A4) are interpreted correctly.
        let width = bitmap.width as i32;
        let height = bitmap.height as i32;
        let bytes_per_pixel = i32::from(bitmap.bytes_per_pixel);
        let bytes_per_line = bitmap.bytes_per_row() as i32;

        let cloned = self
            .api
            .try_clone()
            .map_err(|e| ExtractionError::Internal(format!("tesseract clone: {e:?}")))?;

        cloned
            .set_image(&bitmap.data, width, height, bytes_per_pixel, bytes_per_line)
            .map_err(|e| ExtractionError::Internal(format!("tesseract set_image: {e:?}")))?;

        let text = cloned
            .get_utf8_text()
            .map_err(|e| ExtractionError::Internal(format!("tesseract get_text: {e:?}")))?;

        Ok(text)
    }
}

/// Build an `Arc<dyn OcrProvider>` for wiring/composition when OCR is enabled.
pub fn make_ocr() -> Result<Arc<dyn OcrProvider>, ExtractionError> {
    Ok(Arc::new(TesseractOcrProvider::new()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_inits_or_reports_unavailable() {
        // Only meaningful when the build embedded eng+rus tessdata. Accept
        // either a successful init or a clean OcrUnavailable on hosts without it.
        match TesseractOcrProvider::new() {
            Ok(_) => {}
            Err(e) => assert!(matches!(e, ExtractionError::OcrUnavailable(_))),
        }
    }
}
