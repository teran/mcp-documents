//! `AnyDocPresentationReader` — implements [`PresentationReader`].
//!
//! - **PPTX** (an OOXML zip) is parsed directly: slides are separate
//!   `ppt/slides/slideN.xml` parts, so we can report an accurate slide count and
//!   extract a `slide_range` (pulling `<a:t>` text from each slide's XML).
//! - **Legacy binary `.ppt`** has no separately-addressable slide parts, so it
//!   is routed through `anydoc` as a whole-document unit. `slide_count` is
//!   reported as `1` and `slide_range` is ignored for `.ppt`.

use std::io::{Cursor, Read};
use std::ops::Range;
use std::sync::Arc;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::domain::error::ExtractionError;
use crate::domain::meta::PresentationInfo;
use crate::domain::ports::{DocumentExtractor, PresentationReader};
use crate::domain::result::Source;

use super::error_map::{map_csv_io_error, map_zip_error};
use super::file_reader::read_source_bytes;

/// The OOXML slide part prefix inside a PPTX archive.
const SLIDE_PREFIX: &str = "ppt/slides/slide";
const SLIDE_SUFFIX: &str = ".xml";

/// Extract the `<*:t>` text runs from a slide's XML into a single string.
pub fn slide_xml_to_text(slide_xml: &str) -> Result<String, ExtractionError> {
    let result = std::panic::catch_unwind(|| {
        let mut reader = Reader::from_reader(slide_xml.as_bytes());
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();
        let mut parts: Vec<String> = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Text(ref e)) => {
                    if let Ok(text) = e.decode() {
                        let s = text.trim();
                        if !s.is_empty() {
                            parts.push(s.to_string());
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(_) => break,
            }
            buf.clear();
        }
        Ok::<_, ExtractionError>(parts.join("\n"))
    });
    match result {
        Ok(r) => r,
        Err(_) => Err(ExtractionError::Internal(
            "slide XML parser panicked".into(),
        )),
    }
}

/// Adapter for presentation formats.
#[derive(Default)]
pub struct AnyDocPresentationReader {
    extractor: Option<Arc<dyn DocumentExtractor>>,
}

impl AnyDocPresentationReader {
    /// Create a new reader. Optionally pass an `anydoc`-backed extractor used
    /// for the legacy binary `.ppt` fallback; when omitted a default
    /// [`super::anydoc_extractor::AnyDocExtractor`] is used.
    pub fn new(extractor: Option<Arc<dyn DocumentExtractor>>) -> Self {
        Self { extractor }
    }

    /// Whether the bytes look like a PPTX (OOXML zip) rather than binary `.ppt`.
    fn is_pptx(bytes: &[u8]) -> bool {
        bytes.starts_with(b"PK\x03\x04")
    }

    /// Count `ppt/slides/slideN.xml` parts in a PPTX archive.
    fn pptx_slide_count(bytes: &[u8]) -> Result<usize, ExtractionError> {
        let result = std::panic::catch_unwind(|| {
            let archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec()))?;
            // In zip >= 9 `file_names()` yields `Result<Cow<str>, ZipError>`
            // items (invalid UTF-8 names surface as errors); skip those.
            let count = archive
                .file_names()
                .filter(|n| {
                    n.as_deref()
                        .map(|n| n.starts_with(SLIDE_PREFIX) && n.ends_with(SLIDE_SUFFIX))
                        .unwrap_or(false)
                })
                .count();
            Ok::<usize, zip::result::ZipError>(count)
        });
        match result {
            Ok(Ok(c)) => Ok(c),
            Ok(Err(e)) => Err(map_zip_error(e)),
            Err(_) => Err(ExtractionError::Internal(
                "PPTX archive parser panicked while counting slides".into(),
            )),
        }
    }

    /// Extract the given 1-indexed slide numbers' text from a PPTX archive.
    fn pptx_extract_slides(bytes: &[u8], slide_nums: &[usize]) -> Result<String, ExtractionError> {
        let result = std::panic::catch_unwind(|| {
            let mut archive =
                zip::ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(map_zip_error)?;
            let mut parts: Vec<String> = Vec::new();
            // Cap each decompressed slide part so a zip-bomb member cannot expand
            // to an unbounded string (decompression-bomb guard).
            let max = crate::config::max_decompressed_bytes();
            for &num in slide_nums {
                let name = format!("{SLIDE_PREFIX}{num}{SLIDE_SUFFIX}");
                let file = archive.by_name(&name).map_err(map_zip_error)?;
                let mut limited = file.take(max as u64 + 1);
                let mut xml = String::new();
                limited.read_to_string(&mut xml).map_err(map_csv_io_error)?;
                if xml.len() as u64 > max as u64 {
                    return Err(ExtractionError::ResourceLimit(format!(
                        "slide {num} expands to {} bytes, exceeding MAX_DECOMPRESSED_BYTES={max}",
                        xml.len()
                    )));
                }
                parts.push(slide_xml_to_text(&xml)?);
            }
            Ok::<_, ExtractionError>(parts.join("\n\n"))
        });
        match result {
            Ok(Ok(md)) => Ok(md),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ExtractionError::Internal(
                "PPTX archive parser panicked while extracting slides".into(),
            )),
        }
    }
}

impl PresentationReader for AnyDocPresentationReader {
    fn presentation_info(&self, source: &Source) -> Result<PresentationInfo, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        if Self::is_pptx(&bytes) {
            Ok(PresentationInfo {
                slide_count: Self::pptx_slide_count(&bytes)?,
            })
        } else {
            // Legacy .ppt: whole-document unit; slides not separately addressable.
            Ok(PresentationInfo { slide_count: 1 })
        }
    }

    fn extract_presentation(
        &self,
        source: &Source,
        slide_range: Option<Range<usize>>,
    ) -> Result<String, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        if Self::is_pptx(&bytes) {
            let total = Self::pptx_slide_count(&bytes)?;
            let from = slide_range.as_ref().map(|r| r.start);
            let to = slide_range.as_ref().map(|r| r.end);
            // Resolve against total slides; slide numbers are 1-indexed in the archive.
            let range = crate::domain::ranges::resolve_range(from, to, total, "slide")?;
            let slide_nums: Vec<usize> = range.map(|i| i + 1).collect();
            Self::pptx_extract_slides(&bytes, &slide_nums)
        } else {
            // Legacy .ppt: whole-document anydoc extraction; range ignored.
            let extractor = self
                .extractor
                .clone()
                .unwrap_or_else(|| Arc::new(super::anydoc_extractor::AnyDocExtractor::new()));
            let result = extractor.extract_text(source)?;
            Ok(result.markdown)
        }
    }
}

/// Build an `Arc<dyn PresentationReader>` for wiring/composition.
pub fn make_presentation_reader() -> Arc<dyn PresentationReader> {
    Arc::new(AnyDocPresentationReader::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::MutexGuard;

    use crate::config::ENV_LOCK;

    fn lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Build a minimal in-memory PPTX whose single slide carries `slide_xml`.
    fn make_pptx(slide_xml: &str) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
        zip.write_all(slide_xml.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn slide_xml_to_text_extracts_runs() {
        let xml =
            r#"<p:sld><p:cSld><p:sp><a:t>Hello</a:t><a:t>world</a:t></p:sp></p:cSld></p:sld>"#;
        let text = slide_xml_to_text(xml).unwrap();
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
    }

    #[test]
    fn is_pptx_by_magic() {
        assert!(AnyDocPresentationReader::is_pptx(b"PK\x03\x04 rest"));
        assert!(!AnyDocPresentationReader::is_pptx(&[
            0xD0, 0xCF, 0x11, 0xE0
        ]));
        assert!(!AnyDocPresentationReader::is_pptx(b"garbage"));
    }

    #[test]
    fn constructors() {
        let a = AnyDocPresentationReader::new(None);
        let b = make_presentation_reader();
        let _ = (a, b);
    }

    #[test]
    fn pptx_extracts_slides_and_counts() {
        // Take the env lock so a parallel `pptx_decompression_cap_is_enforced`
        // test that mutates MAX_DECOMPRESSED_BYTES cannot make this read a tiny
        // cap and spuriously fail (env-var race).
        let _g = lock();
        let r = AnyDocPresentationReader::new(None);
        let bytes = make_pptx(r#"<p:sld><a:t>Hi</a:t></p:sld>"#);
        let info = r.presentation_info(&Source::Bytes(bytes.clone())).unwrap();
        assert_eq!(info.slide_count, 1);
        let md = r
            .extract_presentation(&Source::Bytes(bytes), Some(0..1))
            .unwrap();
        assert!(md.contains("Hi"));
    }

    #[test]
    fn pptx_decompression_cap_is_enforced() {
        let _g = lock();
        let r = AnyDocPresentationReader::new(None);
        // A slide XML that decompresses beyond the (tiny) cap.
        let big = format!("<p:sld><a:t>{}</a:t></p:sld>", "x".repeat(4096));
        let bytes = make_pptx(&big);
        unsafe { std::env::set_var("MAX_DECOMPRESSED_BYTES", "16") };
        let err = r
            .extract_presentation(&Source::Bytes(bytes), Some(0..1))
            .unwrap_err();
        unsafe { std::env::remove_var("MAX_DECOMPRESSED_BYTES") };
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));
    }
}
