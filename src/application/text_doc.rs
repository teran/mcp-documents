//! `TextDocUseCase` — word-processor documents (doc / docx / rtf):
//! word/char counting and `word_range` extraction.

use std::ops::Range;
use std::sync::Arc;

use crate::config::max_return_chars;
use crate::domain::document::DocumentKind;
use crate::domain::error::ExtractionError;
use crate::domain::meta::TextDocumentInfo;
use crate::domain::ports::{DocumentExtractor, TextDocumentReader};
use crate::domain::ranges::resolve_range;
use crate::domain::result::{ExtractionResult, Source};

/// Orchestrates word/char counting and word-range extraction for word-processor
/// documents.
#[derive(Clone)]
pub struct TextDocUseCase {
    classifier: Arc<dyn DocumentExtractor>,
    reader: Arc<dyn TextDocumentReader>,
    /// Safety-net cap on the size of a returned word-range chunk.
    max_return_chars: usize,
}

impl TextDocUseCase {
    /// Build the use case around its injected ports, reading the char cap from
    /// env (`MAX_RETURN_CHARS`).
    pub fn new(
        classifier: Arc<dyn DocumentExtractor>,
        reader: Arc<dyn TextDocumentReader>,
    ) -> Self {
        Self::new_with_max_chars(classifier, reader, max_return_chars())
    }

    /// Build with an explicit char cap (used by tests to avoid env races).
    pub fn new_with_max_chars(
        classifier: Arc<dyn DocumentExtractor>,
        reader: Arc<dyn TextDocumentReader>,
        max_return_chars: usize,
    ) -> Self {
        Self {
            classifier,
            reader,
            max_return_chars,
        }
    }

    /// Verify the source is exactly `expected` (e.g. `Docx` for `docx_info`),
    /// returning a clear tool-level error otherwise.
    fn require_kind(
        &self,
        source: &Source,
        expected: DocumentKind,
    ) -> Result<DocumentKind, ExtractionError> {
        let c = self.classifier.classify(source)?;
        if c.kind != expected {
            return Err(ExtractionError::Unsupported(c.kind));
        }
        Ok(c.kind)
    }

    /// Word and character counts for the whole document of kind `expected`.
    pub fn info(
        &self,
        source: &Source,
        expected: DocumentKind,
    ) -> Result<TextDocumentInfo, ExtractionError> {
        let _kind = self.require_kind(source, expected)?;
        self.reader.text_info(source)
    }

    /// Extract the requested 0-based `word_range` (default all words) as
    /// Markdown for a document of kind `expected`. Enforces a per-request
    /// character cap on the returned chunk.
    pub fn extract_markdown(
        &self,
        source: &Source,
        expected: DocumentKind,
        word_range: Option<Range<usize>>,
    ) -> Result<ExtractionResult, ExtractionError> {
        let kind = self.require_kind(source, expected)?;
        let text = self.reader.extract_text(source)?;
        let words: Vec<&str> = text.split_whitespace().collect();

        let from = word_range.as_ref().map(|r| r.start);
        let to = word_range.as_ref().map(|r| r.end);
        let range = resolve_range(from, to, words.len(), "word")?;

        let chunk = words[range].join(" ");
        let char_count = chunk.chars().count();
        if char_count > self.max_return_chars {
            return Err(ExtractionError::ResourceLimit(format!(
                "word range expands to {char_count} characters, exceeding MAX_RETURN_CHARS={}",
                self.max_return_chars
            )));
        }

        Ok(ExtractionResult::markdown_only(kind, None, chunk))
    }
}

/// Split extracted text into whitespace-delimited words (pure, testable).
pub fn split_words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::document::{Classification, PdfType};

    #[derive(Clone)]
    struct FakeClassifier {
        kind: DocumentKind,
    }
    impl DocumentExtractor for FakeClassifier {
        fn classify(&self, _source: &Source) -> Result<Classification, ExtractionError> {
            let pdf_type = if self.kind == DocumentKind::Pdf {
                Some(PdfType::TextBased)
            } else {
                None
            };
            Ok(Classification {
                kind: self.kind,
                pdf_type,
                pages_needing_ocr: Vec::new(),
            })
        }
        fn extract_text(&self, _source: &Source) -> Result<ExtractionResult, ExtractionError> {
            unreachable!()
        }
    }

    #[derive(Clone)]
    struct FakeReader {
        text: String,
    }
    impl TextDocumentReader for FakeReader {
        fn text_info(&self, _source: &Source) -> Result<TextDocumentInfo, ExtractionError> {
            let words = split_words(&self.text).len();
            Ok(TextDocumentInfo {
                word_count: words,
                char_count: self.text.chars().count(),
            })
        }
        fn extract_text(&self, _source: &Source) -> Result<String, ExtractionError> {
            Ok(self.text.clone())
        }
    }

    fn uc(kind: DocumentKind, text: &str) -> TextDocUseCase {
        TextDocUseCase::new(
            Arc::new(FakeClassifier { kind }),
            Arc::new(FakeReader {
                text: text.to_string(),
            }),
        )
    }

    #[test]
    fn info_returns_counts() {
        let u = uc(DocumentKind::Docx, "hello world foo");
        let info = u.info(&Source::Bytes(vec![1]), DocumentKind::Docx).unwrap();
        assert_eq!(info.word_count, 3);
        assert_eq!(info.char_count, 15);
    }

    #[test]
    fn non_word_processor_kind_is_rejected() {
        let u = uc(DocumentKind::Xlsx, "x");
        let err = u
            .info(&Source::Bytes(vec![1]), DocumentKind::Docx)
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Unsupported(_)));
    }

    #[test]
    fn full_word_range_returns_all_words() {
        let u = uc(DocumentKind::Doc, "one two three four");
        let out = u
            .extract_markdown(&Source::Bytes(vec![1]), DocumentKind::Doc, None)
            .unwrap();
        assert_eq!(out.markdown, "one two three four");
    }

    #[test]
    fn word_range_slices_words() {
        let u = uc(DocumentKind::Rtf, "one two three four");
        let out = u
            .extract_markdown(&Source::Bytes(vec![1]), DocumentKind::Rtf, Some(1..3))
            .unwrap();
        assert_eq!(out.markdown, "two three");
    }

    #[test]
    fn char_cap_is_enforced() {
        let u = TextDocUseCase::new_with_max_chars(
            Arc::new(FakeClassifier {
                kind: DocumentKind::Docx,
            }),
            Arc::new(FakeReader {
                text: "a b c d e f g h".to_string(),
            }),
            5,
        );
        let err = u
            .extract_markdown(&Source::Bytes(vec![1]), DocumentKind::Docx, None)
            .unwrap_err();
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));
    }

    #[test]
    fn char_cap_at_exact_limit_succeeds() {
        // "a b c" joins to a 5-char chunk. Under the strict `char_count > max`
        // comparison equality is allowed, so the request succeeds; a `>` → `>=`
        // mutant would reject the exact-fit chunk and this test would fail.
        let u = TextDocUseCase::new_with_max_chars(
            Arc::new(FakeClassifier {
                kind: DocumentKind::Docx,
            }),
            Arc::new(FakeReader {
                text: "a b c".to_string(),
            }),
            5,
        );
        let out = u
            .extract_markdown(&Source::Bytes(vec![1]), DocumentKind::Docx, None)
            .unwrap();
        assert_eq!(out.markdown, "a b c");
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges)]
    fn from_greater_than_to_is_malformed() {
        let u = uc(DocumentKind::Docx, "a b c");
        let err = u
            .extract_markdown(&Source::Bytes(vec![1]), DocumentKind::Docx, Some(3..1))
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Malformed(_)));
    }

    #[test]
    fn split_words_handles_whitespace() {
        assert_eq!(split_words("a  b\tc\n d"), vec!["a", "b", "c", "d"]);
        assert!(split_words("   ").is_empty());
    }
}
