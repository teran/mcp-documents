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
//! a **cache directory** (`get_embedded_tessdata` / `embedded_languages`) and
//! call `api.init(cache_dir, "eng+rus")`. The bytes come from the embedded
//! binary, not disk/network, preserving the zero-runtime-dependency property.
//!
//! # Startup cache
//! The extracted `.traineddata` files are treated as a **cache**: after the
//! first materialization they are not re-extracted on subsequent startups.
//! Each language file is only written when it is missing or its on-disk size
//! differs from the embedded byte length — otherwise we skip with a cheap
//! `metadata().len()` stat. This makes server startup fast after the first run.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::domain::error::ExtractionError;
use crate::domain::ports::OcrProvider;
use crate::domain::result::PageBitmap;

/// The stable per-user cache directory that survives across runs.
///
/// We prefer an XDG-style user cache dir (`$XDG_CACHE_HOME` or
/// `~/.cache/mcp-documents/tessdata`); on macOS `HOME` points at `/Users/<u>`
/// so this resolves to `~/Library/Caches/...` only if the OS routes
/// `XDG_CACHE_HOME` there (it usually does not, but the location is stable
/// regardless). Falls back to a fixed (non-PID) temp dir name otherwise.
fn cache_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("mcp-documents/tessdata");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join(".cache/mcp-documents/tessdata");
    }
    std::env::temp_dir().join("mcp-documents-tessdata")
}

/// Ensure each embedded language's `.traineddata` exists in `cache_dir` with
/// the correct size.
///
/// Fast path: if a file already exists **and** its length equals the embedded
/// byte length, it is skipped (just a `metadata()` stat) — no re-extraction.
///
/// Concurrency: the write path is only hit when a file is missing or has the
/// wrong size. Writes are **idempotent** (identical embedded bytes) and go
/// straight to the final path; a torn write is self-healing because a
/// wrong-size file is detected and rewritten on the next startup. For
/// simplicity we do not take a lock — concurrent providers writing the same
/// bytes is safe.
fn ensure_embedded_tessdata(cache_dir: &Path) -> Result<(), ExtractionError> {
    for lang in tesseract_rs::embedded_languages() {
        let Some(bytes) = tesseract_rs::get_embedded_tessdata(lang) else {
            continue;
        };
        let path = cache_dir.join(format!("{lang}.traineddata"));

        // Fast path: file present with the exact embedded size -> keep it.
        let cached_len = std::fs::metadata(&path).map(|m| m.len()).ok();
        if cached_len == Some(bytes.len() as u64) {
            continue;
        }

        std::fs::write(&path, bytes)
            .map_err(|e| ExtractionError::Io(format!("write {lang}.traineddata: {e}")))?;
    }
    Ok(())
}

/// A Tesseract-backed OCR provider consuming rendered page bitmaps.
///
/// `new` performs engine init once, materializing embedded tessdata into the
/// stable cache dir (skipping files that are already cached). Keep a single
/// instance and clone per worker via [`TesseractAPI::try_clone`] for parallel
/// OCR.
pub struct TesseractOcrProvider {
    api: tesseract_rs::TesseractAPI,
    /// Kept alive for the lifetime of the provider (the cache dir containing
    /// the materialized `.traineddata` files).
    _cache_dir: PathBuf,
}

impl TesseractOcrProvider {
    /// Initialize the engine once (embedded eng+rus tessdata cached in a stable
    /// per-user dir). Returns `OcrUnavailable` if the engine cannot be
    /// initialized.
    pub fn new() -> Result<Self, ExtractionError> {
        Self::new_with_languages("eng+rus")
    }

    /// Initialize with an explicit language spec.
    pub fn new_with_languages(langs: &str) -> Result<Self, ExtractionError> {
        Self::new_with_cache_dir(cache_dir(), langs)
    }

    /// Initialize against an explicit cache directory (used by tests to avoid
    /// touching the real user cache).
    pub fn new_with_cache_dir(cache_dir: PathBuf, langs: &str) -> Result<Self, ExtractionError> {
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| ExtractionError::Io(format!("create tessdata dir: {e}")))?;
        ensure_embedded_tessdata(&cache_dir)?;

        let api = tesseract_rs::TesseractAPI::new();
        api.init(&cache_dir, langs)
            .map_err(|e| ExtractionError::OcrUnavailable(format!("tesseract init: {e:?}")))?;

        Ok(Self {
            api,
            _cache_dir: cache_dir,
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
    use crate::domain::result::PageBitmap;
    use std::time::{Duration, SystemTime};

    /// A unique scratch cache dir for a test, cleaned up on drop.
    struct TestDir(PathBuf);
    impl TestDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mcp-tess-test-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Obtain a live provider, or assert init is unavailable on a host without
    /// tessdata (so tests don't hard-fail on a misconfigured machine).
    fn provider_or_skip() -> Option<TesseractOcrProvider> {
        match TesseractOcrProvider::new() {
            Ok(p) => Some(p),
            Err(e) => {
                assert!(
                    matches!(e, ExtractionError::OcrUnavailable(_)),
                    "unexpected init failure: {e}"
                );
                None
            }
        }
    }

    #[test]
    fn constructor_inits_or_reports_unavailable() {
        // Only meaningful when the build embedded eng+rus tessdata. Accept
        // either a successful init or a clean OcrUnavailable on hosts without it.
        match TesseractOcrProvider::new() {
            Ok(_) => {}
            Err(e) => assert!(matches!(e, ExtractionError::OcrUnavailable(_))),
        }
    }

    #[test]
    fn new_with_languages_inits_or_reports_unavailable() {
        match TesseractOcrProvider::new_with_languages("eng+rus") {
            Ok(_) => {}
            Err(e) => assert!(matches!(e, ExtractionError::OcrUnavailable(_))),
        }
    }

    #[test]
    fn ocr_page_zero_width_bitmap_is_internal_error() {
        // A zero-width page passes PageBitmap validation (empty buffer) but is
        // rejected by Tesseract's set_image (width <= 0) and must map to a
        // domain Internal error — never a panic.
        let Some(provider) = provider_or_skip() else {
            return;
        };
        let bmp = PageBitmap::new(0, 100, 1, Vec::new()).unwrap();
        let err = provider.ocr_page(&bmp).unwrap_err();
        assert!(matches!(err, ExtractionError::Internal(_)));
    }

    #[test]
    fn ocr_page_success_returns_text() {
        let Some(provider) = provider_or_skip() else {
            return;
        };
        let bmp = PageBitmap::new(16, 16, 1, vec![255u8; 16 * 16]).unwrap();
        assert!(provider.ocr_page(&bmp).is_ok());
    }

    #[test]
    fn make_ocr_returns_working_provider() {
        let Some(provider) = make_ocr().ok() else {
            return;
        };
        let bmp = PageBitmap::new(16, 16, 1, vec![255u8; 16 * 16]).unwrap();
        assert!(provider.ocr_page(&bmp).is_ok());
    }

    // --- tessdata cache behavior ---

    /// Size of an embedded language's bytes, or None if not embedded.
    fn embedded_len(lang: &str) -> Option<usize> {
        tesseract_rs::get_embedded_tessdata(lang).map(|b| b.len())
    }

    #[test]
    fn cache_dir_is_created_on_demand() {
        let dir = TestDir::new("created");
        // Remove the dir to prove the provider recreates it on demand.
        std::fs::remove_dir_all(dir.path()).unwrap();
        assert!(!dir.path().exists());

        if TesseractOcrProvider::new_with_cache_dir(dir.path().to_path_buf(), "eng+rus").is_ok() {
            assert!(dir.path().is_dir(), "cache dir must be created on demand");
        }
        // On a host without tessdata the init returns OcrUnavailable and there
        // is nothing further to assert.
    }

    #[test]
    fn first_materialization_writes_all_languages() {
        let dir = TestDir::new("materialize");
        ensure_embedded_tessdata(dir.path()).unwrap();
        for lang in tesseract_rs::embedded_languages() {
            let Some(len) = embedded_len(lang) else {
                continue;
            };
            let path = dir.path().join(format!("{lang}.traineddata"));
            let m = std::fs::metadata(&path).unwrap();
            assert_eq!(
                m.len(),
                len as u64,
                "{lang} should be materialized at correct size"
            );
        }
    }

    #[test]
    fn second_materialization_skips_matching_files_fast_path() {
        let dir = TestDir::new("fastpath");
        ensure_embedded_tessdata(dir.path()).unwrap();

        // Pick the first embedded language and pin its mtime to a known past
        // instant. If the fast path skips the rewrite, the mtime must be
        // unchanged after a second materialization.
        let Some(lang) = tesseract_rs::embedded_languages().into_iter().next() else {
            return;
        };
        let path = dir.path().join(format!("{lang}.traineddata"));
        let past = SystemTime::now() - Duration::from_secs(3600);
        {
            let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.set_modified(past).unwrap();
        }

        ensure_embedded_tessdata(dir.path()).unwrap();

        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            after, past,
            "file must NOT be rewritten when present with matching size"
        );
    }

    #[test]
    fn mismatched_size_is_rewritten() {
        let dir = TestDir::new("rewrite");
        let Some(lang) = tesseract_rs::embedded_languages().into_iter().next() else {
            return;
        };
        let Some(len) = embedded_len(lang) else {
            return;
        };

        // Write a sentinel file with the WRONG size.
        let path = dir.path().join(format!("{lang}.traineddata"));
        std::fs::write(&path, b"SENTINEL-TRUNCATED").unwrap();
        assert_ne!(std::fs::metadata(&path).unwrap().len(), len as u64);

        ensure_embedded_tessdata(dir.path()).unwrap();

        // Now it must have been rewritten to the correct embedded size.
        assert_eq!(std::fs::metadata(&path).unwrap().len(), len as u64);
    }

    #[test]
    fn second_provider_init_uses_cache_without_rewrite() {
        // Initialize a provider into a scratch dir, pin mtime, re-initialize a
        // second provider from the same dir, and confirm the cache file was not
        // rewritten (i.e. startup uses the fast path).
        let dir = TestDir::new("init");
        let cache = dir.path().to_path_buf();

        let p1 = match TesseractOcrProvider::new_with_cache_dir(cache.clone(), "eng+rus") {
            Ok(p) => p,
            Err(_) => return, // no tessdata on this host
        };
        drop(p1);

        let Some(lang) = tesseract_rs::embedded_languages().into_iter().next() else {
            return;
        };
        let path = cache.join(format!("{lang}.traineddata"));
        let past = SystemTime::now() - Duration::from_secs(7200);
        {
            let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.set_modified(past).unwrap();
        }

        let p2 = TesseractOcrProvider::new_with_cache_dir(cache.clone(), "eng+rus")
            .expect("second init should succeed");
        drop(p2);

        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            after, past,
            "second startup must NOT re-extract cached tessdata"
        );
    }
}
