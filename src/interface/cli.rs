//! Standalone CLI over the same application use cases (for manual testing
//! without an MCP client).

use std::path::PathBuf;

use crate::application::{ClassifyDocumentUseCase, ExtractDocumentUseCase};
use crate::domain::result::Source;

/// A recognized CLI subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    /// `classify <path>` — print the PDF classification.
    Classify(PathBuf),
    /// `extract <path>` — print extracted Markdown.
    Extract(PathBuf),
    /// `extract-text <path>` — print extracted plain text.
    ExtractText(PathBuf),
    /// `serve` — run the MCP stdio server.
    Serve,
}

impl CliCommand {
    /// Parse the first CLI argument into a command. Returns `None` when the
    /// first argument is not a recognized subcommand (the caller then falls
    /// back to serving MCP over stdio).
    pub fn parse(args: &[String]) -> Option<CliCommand> {
        let first = args.first()?;
        match first.as_str() {
            "classify" => args.get(1).map(|p| CliCommand::Classify(PathBuf::from(p))),
            "extract" => args.get(1).map(|p| CliCommand::Extract(PathBuf::from(p))),
            "extract-text" => args
                .get(1)
                .map(|p| CliCommand::ExtractText(PathBuf::from(p))),
            "serve" => Some(CliCommand::Serve),
            _ => None,
        }
    }

    /// Whether this command is the MCP stdio server.
    pub fn is_serve(&self) -> bool {
        matches!(self, CliCommand::Serve)
    }
}

/// Execute a CLI command and print the result to stdout.
pub async fn run_cli(
    cmd: &CliCommand,
    classify: &ClassifyDocumentUseCase,
    extract: &ExtractDocumentUseCase,
) -> Result<(), String> {
    match cmd {
        CliCommand::Classify(path) => {
            let source = Source::Path(path.clone());
            let c = classify.classify(&source).map_err(|e| e.to_string())?;
            let pdf_type = c
                .pdf_type
                .map(|t| format!("{t:?}"))
                .unwrap_or_else(|| "none".into());
            println!("kind: {}", c.kind.as_str());
            println!("pdf_type: {pdf_type}");
            for p in &c.pages_needing_ocr {
                println!(
                    "page {}: needs_ocr={} confidence={:.2}",
                    p.page_index, p.needs_ocr, p.confidence
                );
            }
            Ok(())
        }
        CliCommand::Extract(path) | CliCommand::ExtractText(path) => {
            let source = Source::Path(path.clone());
            let result = extract.extract(&source).await.map_err(|e| e.to_string())?;
            println!("{}", result.markdown);
            Ok(())
        }
        CliCommand::Serve => {
            // Handled by main; reaching here is a programming error.
            Err("'serve' must be run through the MCP entrypoint".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_subcommands() {
        assert_eq!(
            CliCommand::parse(&args(&["classify", "a.pdf"])),
            Some(CliCommand::Classify(PathBuf::from("a.pdf")))
        );
        assert_eq!(
            CliCommand::parse(&args(&["extract", "b.pdf"])),
            Some(CliCommand::Extract(PathBuf::from("b.pdf")))
        );
        assert_eq!(
            CliCommand::parse(&args(&["extract-text", "c.pdf"])),
            Some(CliCommand::ExtractText(PathBuf::from("c.pdf")))
        );
        assert_eq!(
            CliCommand::parse(&args(&["serve"])),
            Some(CliCommand::Serve)
        );
        assert!(CliCommand::parse(&args(&["classify"])).is_none());
        assert!(CliCommand::parse(&args(&["unknown", "x"])).is_none());
        assert!(CliCommand::parse(&args(&[])).is_none());
    }

    #[test]
    fn is_serve_flag() {
        assert!(CliCommand::Serve.is_serve());
        assert!(!CliCommand::Classify(PathBuf::from("x")).is_serve());
    }

    // --- run_cli (fake ports) ---

    use std::sync::Arc;

    use crate::application::{ClassifyDocumentUseCase, ExtractDocumentUseCase};
    use crate::domain::document::{Classification, DocumentKind, PdfType};
    use crate::domain::error::ExtractionError;
    use crate::domain::ports::{DocumentExtractor, OcrProvider, PdfPageRenderer};
    use crate::domain::result::{ExtractionResult, PageBitmap, Source};

    /// Fake extractor classifying input as the given kind.
    #[derive(Clone)]
    struct FakeExtractor {
        kind: DocumentKind,
    }

    impl Default for FakeExtractor {
        fn default() -> Self {
            Self {
                kind: DocumentKind::Pdf,
            }
        }
    }

    impl DocumentExtractor for FakeExtractor {
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
            Ok(ExtractionResult::markdown_only(
                self.kind,
                None,
                "cli markdown".into(),
            ))
        }
    }

    struct FakeOcr;
    impl OcrProvider for FakeOcr {
        fn ocr_page(&self, _b: &PageBitmap) -> Result<String, ExtractionError> {
            Err(ExtractionError::OcrUnavailable("no".into()))
        }
    }

    struct FakeRenderer;
    impl PdfPageRenderer for FakeRenderer {
        fn render_page(
            &self,
            _p: &[u8],
            _i: usize,
            _d: u32,
        ) -> Result<PageBitmap, ExtractionError> {
            Err(ExtractionError::Internal("no render".into()))
        }
    }

    fn use_cases_with(kind: DocumentKind) -> (ClassifyDocumentUseCase, ExtractDocumentUseCase) {
        let ext = Arc::new(FakeExtractor { kind });
        let classify = ClassifyDocumentUseCase::new(ext.clone());
        let extract = ExtractDocumentUseCase::new(ext, Arc::new(FakeOcr), Arc::new(FakeRenderer));
        (classify, extract)
    }

    fn use_cases() -> (ClassifyDocumentUseCase, ExtractDocumentUseCase) {
        use_cases_with(DocumentKind::Pdf)
    }

    #[tokio::test]
    async fn run_cli_classify_prints_and_ok() {
        let (classify, extract) = use_cases();
        let cmd = CliCommand::Classify(PathBuf::from("a.pdf"));
        assert!(run_cli(&cmd, &classify, &extract).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_and_extract_text_ok() {
        let (classify, extract) = use_cases();
        for cmd in [
            CliCommand::Extract(PathBuf::from("a.pdf")),
            CliCommand::ExtractText(PathBuf::from("a.pdf")),
        ] {
            assert!(run_cli(&cmd, &classify, &extract).await.is_ok());
        }
    }

    #[tokio::test]
    async fn run_cli_classify_non_pdf_ok() {
        let (classify, extract) = use_cases_with(DocumentKind::Docx);
        let cmd = CliCommand::Classify(PathBuf::from("b.docx"));
        assert!(run_cli(&cmd, &classify, &extract).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_non_pdf_ok() {
        let (classify, extract) = use_cases_with(DocumentKind::Xlsx);
        let cmd = CliCommand::Extract(PathBuf::from("c.xlsx"));
        assert!(run_cli(&cmd, &classify, &extract).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_serve_returns_error() {
        let (classify, extract) = use_cases();
        let cmd = CliCommand::Serve;
        assert!(run_cli(&cmd, &classify, &extract).await.is_err());
    }
}
