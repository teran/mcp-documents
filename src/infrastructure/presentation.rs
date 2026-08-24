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
            let count = archive
                .file_names()
                .filter(|n| n.starts_with(SLIDE_PREFIX) && n.ends_with(SLIDE_SUFFIX))
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
            for &num in slide_nums {
                let name = format!("{SLIDE_PREFIX}{num}{SLIDE_SUFFIX}");
                let mut file = archive.by_name(&name).map_err(map_zip_error)?;
                let mut xml = String::new();
                file.read_to_string(&mut xml).map_err(map_csv_io_error)?;
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
}
