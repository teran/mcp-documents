//! `AnyDocTextReader` — implements [`TextDocumentReader`] using `anydoc`.
//!
//! Word-processor formats (doc / docx / rtf) have no real page entity, so this
//! adapter extracts the whole document's Markdown via `anydoc` and computes
//! word/char counts from it (a word is whitespace-delimited).

use std::sync::Arc;

use crate::domain::error::ExtractionError;
use crate::domain::meta::TextDocumentInfo;
use crate::domain::ports::TextDocumentReader;
use crate::domain::result::Source;

use super::error_map::map_convert_error;
use super::file_reader::read_source_bytes;

/// Adapter computing word/char counts for word-processor documents.
#[derive(Default)]
pub struct AnyDocTextReader {}

impl AnyDocTextReader {
    /// Create a new reader.
    pub fn new() -> Self {
        Self {}
    }
}

impl TextDocumentReader for AnyDocTextReader {
    fn text_info(&self, source: &Source) -> Result<TextDocumentInfo, ExtractionError> {
        let text = self.extract_text(source)?;
        let word_count = text.split_whitespace().count();
        let char_count = text.chars().count();
        Ok(TextDocumentInfo {
            word_count,
            char_count,
        })
    }

    fn extract_text(&self, source: &Source) -> Result<String, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let result = std::panic::catch_unwind(|| anydoc::to_markdown_bytes(&bytes, None));
        match result {
            Ok(Ok(text)) => Ok(text),
            Ok(Err(e)) => Err(map_convert_error(e)),
            Err(_) => Err(ExtractionError::Internal(
                "document conversion panicked while extracting text".into(),
            )),
        }
    }
}

/// Build an `Arc<dyn TextDocumentReader>` for wiring/composition.
pub fn make_text_doc_reader() -> Arc<dyn TextDocumentReader> {
    Arc::new(AnyDocTextReader::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors() {
        let a = AnyDocTextReader::new();
        let b = make_text_doc_reader();
        let _ = (a, b);
    }

    #[test]
    fn word_char_counts_from_text() {
        // Exercise text_info with a non-PDF source that anydoc can convert; the
        // real counts are verified against committed fixtures in tests/.
        let r = AnyDocTextReader::new();
        // A minimal CSV is handled by anydoc as a non-word-processor format, so
        // this only proves the counting path runs without panicking.
        let _ = r.text_info(&Source::Bytes(b"a,b,c\n1,2,3".to_vec()));
    }
}
