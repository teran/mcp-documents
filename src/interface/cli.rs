//! Standalone CLI over the same application use cases (for manual testing
//! without an MCP client).

use std::path::PathBuf;

use crate::application::{
    ClassifyDocumentUseCase, CsvUseCase, PdfUseCase, PresentationUseCase, SpreadsheetUseCase,
    TextDocUseCase,
};
use crate::domain::document::DocumentKind;
use crate::domain::result::Source;

/// A recognized CLI subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    /// `classify <path>` — print the classification.
    Classify(PathBuf),
    /// `extract <path>` — print extracted Markdown.
    Extract(PathBuf),
    /// `serve` — run the MCP stdio server.
    Serve,
}

/// The outcome of argv dispatch: either a CLI command to run, or the MCP
/// stdio server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliDispatch {
    /// Run the given CLI command.
    Cli(CliCommand),
    /// Fall through to the MCP stdio server (the `serve` subcommand, an
    /// unknown subcommand, or no arguments).
    Server,
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
            "serve" => Some(CliCommand::Serve),
            _ => None,
        }
    }

    /// Whether this command is the MCP stdio server.
    pub fn is_serve(&self) -> bool {
        matches!(self, CliCommand::Serve)
    }

    /// Decide whether argv selects a CLI command to run or falls through to the
    /// MCP stdio server.
    ///
    /// This mirrors the `main` dispatch decision so it can be unit-tested
    /// without invoking the binary or the MCP transport. A recognized
    /// `classify`/`extract` subcommand yields [`CliDispatch::Cli`]; everything
    /// else (`serve`, an unknown subcommand, or no arguments) yields
    /// [`CliDispatch::Server`].
    pub fn dispatch(args: &[String]) -> CliDispatch {
        match Self::parse(args) {
            Some(cmd) if !cmd.is_serve() => CliDispatch::Cli(cmd),
            _ => CliDispatch::Server,
        }
    }
}

/// Execute a CLI command and print the result to stdout.
#[allow(clippy::too_many_arguments)]
pub async fn run_cli(
    cmd: &CliCommand,
    classify: &ClassifyDocumentUseCase,
    pdf: &PdfUseCase,
    text_doc: &TextDocUseCase,
    spreadsheet: &SpreadsheetUseCase,
    presentation: &PresentationUseCase,
    csv: &CsvUseCase,
) -> Result<(), String> {
    match cmd {
        CliCommand::Classify(path) => {
            let source = Source::Path(path.clone());
            let c = classify.classify(&source).map_err(|e| e.to_string())?;
            let pdf_type = c
                .pdf_type
                .map(|t| t.as_str().to_string())
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
        CliCommand::Extract(path) => {
            let source = Source::Path(path.clone());
            let c = classify.classify(&source).map_err(|e| e.to_string())?;
            let markdown = match c.kind {
                DocumentKind::Pdf => {
                    pdf.extract_markdown(&source, None)
                        .await
                        .map_err(|e| e.to_string())?
                        .markdown
                }
                k @ (DocumentKind::Doc | DocumentKind::Docx | DocumentKind::Rtf) => {
                    text_doc
                        .extract_markdown(&source, k, None)
                        .map_err(|e| e.to_string())?
                        .markdown
                }
                k @ (DocumentKind::Xls | DocumentKind::Xlsx) => {
                    spreadsheet
                        .extract_markdown(&source, k, None, None)
                        .map_err(|e| e.to_string())?
                        .markdown
                }
                k @ (DocumentKind::Ppt | DocumentKind::Pptx) => {
                    presentation
                        .extract_markdown(&source, k, None)
                        .map_err(|e| e.to_string())?
                        .markdown
                }
                DocumentKind::Csv => {
                    csv.extract_markdown(&source, None)
                        .map_err(|e| e.to_string())?
                        .markdown
                }
                // Fallback: whole-document extraction via the classifier's
                // anydoc path (e.g. ODF, EPUB).
                other => c_kind_fallback(classify, &source, other)?,
            };
            println!("{markdown}");
            Ok(())
        }
        CliCommand::Serve => {
            // Handled by main; reaching here is a programming error.
            Err("'serve' must be run through the MCP entrypoint".into())
        }
    }
}

/// Whole-document fallback for kinds not covered by a dedicated use case
/// (ODF/EPUB), using the classifier's `anydoc` extraction path.
fn c_kind_fallback(
    classify: &ClassifyDocumentUseCase,
    source: &Source,
    _kind: DocumentKind,
) -> Result<String, String> {
    let result = classify.extract_text(source).map_err(|e| e.to_string())?;
    Ok(result.markdown)
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

    #[test]
    fn dispatch_routes_recognized_subcommands_to_cli() {
        assert_eq!(
            CliCommand::dispatch(&args(&["classify", "a.pdf"])),
            CliDispatch::Cli(CliCommand::Classify(PathBuf::from("a.pdf")))
        );
        assert_eq!(
            CliCommand::dispatch(&args(&["extract", "b.docx"])),
            CliDispatch::Cli(CliCommand::Extract(PathBuf::from("b.docx")))
        );
    }

    #[test]
    fn dispatch_routes_serve_to_server() {
        assert_eq!(CliCommand::dispatch(&args(&["serve"])), CliDispatch::Server);
    }

    #[test]
    fn dispatch_routes_unknown_and_no_args_to_server() {
        assert_eq!(
            CliCommand::dispatch(&args(&["unknown", "x"])),
            CliDispatch::Server
        );
        assert_eq!(CliCommand::dispatch(&args(&[])), CliDispatch::Server);
    }

    #[test]
    fn dispatch_routes_subcommand_without_path_to_server() {
        // `classify` with no path has no command to run, so it falls through to
        // the server (matching the existing `parse` behavior).
        assert_eq!(
            CliCommand::dispatch(&args(&["classify"])),
            CliDispatch::Server
        );
        assert_eq!(
            CliCommand::dispatch(&args(&["extract"])),
            CliDispatch::Server
        );
    }

    // --- run_cli (fake ports) ---

    use std::sync::Arc;

    use crate::application::{
        ClassifyDocumentUseCase, CsvUseCase, PdfUseCase, PresentationUseCase, SpreadsheetUseCase,
        TextDocUseCase,
    };
    use crate::domain::document::{Classification, PdfType};
    use crate::domain::error::ExtractionError;
    use crate::domain::meta::{
        CsvInfo, PdfInfo, PresentationInfo, SpreadsheetInfo, TextDocumentInfo,
    };
    use crate::domain::ports::{
        CsvReader, DocumentExtractor, OcrProvider, PdfPageRenderer, PdfTextSource,
        PresentationReader, SpreadsheetReader, TextDocumentReader,
    };
    use crate::domain::result::{ExtractionResult, PageBitmap, Source};

    #[derive(Clone)]
    struct FakeClassifier {
        kind: DocumentKind,
        error: Option<ExtractionError>,
    }
    impl DocumentExtractor for FakeClassifier {
        fn classify(&self, _source: &Source) -> Result<Classification, ExtractionError> {
            if let Some(e) = &self.error {
                return Err(e.clone());
            }
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
                "fallback markdown".into(),
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
    struct FakePdfText;
    impl PdfTextSource for FakePdfText {
        fn pdf_info(&self, _s: &Source) -> Result<PdfInfo, ExtractionError> {
            Ok(PdfInfo {
                page_count: 1,
                pdf_type: PdfType::TextBased,
                pages_needing_ocr: Vec::new(),
            })
        }
        fn extract_pages(
            &self,
            _s: &Source,
            _p: &[usize],
        ) -> Result<Vec<crate::domain::result::ExtractedPage>, ExtractionError> {
            Ok(vec![crate::domain::result::ExtractedPage::local(
                0, "pdf text",
            )])
        }
    }
    struct FakeTextReader;
    impl TextDocumentReader for FakeTextReader {
        fn text_info(&self, _s: &Source) -> Result<TextDocumentInfo, ExtractionError> {
            Ok(TextDocumentInfo {
                word_count: 1,
                char_count: 4,
            })
        }
        fn extract_text(&self, _s: &Source) -> Result<String, ExtractionError> {
            Ok("doc markdown".into())
        }
    }
    struct FakeSheetReader;
    impl SpreadsheetReader for FakeSheetReader {
        fn spreadsheet_info(&self, _s: &Source) -> Result<SpreadsheetInfo, ExtractionError> {
            Ok(SpreadsheetInfo { sheets: vec![] })
        }
        fn extract_sheet(
            &self,
            _s: &Source,
            _sh: Option<&str>,
            _r: Option<std::ops::Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("sheet markdown".into())
        }
    }
    struct FakePresReader;
    impl PresentationReader for FakePresReader {
        fn presentation_info(&self, _s: &Source) -> Result<PresentationInfo, ExtractionError> {
            Ok(PresentationInfo { slide_count: 1 })
        }
        fn extract_presentation(
            &self,
            _s: &Source,
            _r: Option<std::ops::Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("slide markdown".into())
        }
    }
    struct FakeCsvReader;
    impl CsvReader for FakeCsvReader {
        fn csv_info(&self, _s: &Source) -> Result<CsvInfo, ExtractionError> {
            Ok(CsvInfo {
                row_count: 1,
                column_count: 1,
                header: vec![],
            })
        }
        fn extract_csv(
            &self,
            _s: &Source,
            _r: Option<std::ops::Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("csv markdown".into())
        }
    }

    fn use_cases(
        kind: DocumentKind,
    ) -> (
        ClassifyDocumentUseCase,
        PdfUseCase,
        TextDocUseCase,
        SpreadsheetUseCase,
        PresentationUseCase,
        CsvUseCase,
    ) {
        let ext = Arc::new(FakeClassifier { kind, error: None });
        let classify = ClassifyDocumentUseCase::new(ext.clone());
        let pdf = PdfUseCase::new(
            ext.clone(),
            Arc::new(FakePdfText),
            Arc::new(FakeRenderer),
            Arc::new(FakeOcr),
        );
        let text_doc = TextDocUseCase::new(ext.clone(), Arc::new(FakeTextReader));
        let spreadsheet = SpreadsheetUseCase::new(ext.clone(), Arc::new(FakeSheetReader));
        let presentation = PresentationUseCase::new(ext.clone(), Arc::new(FakePresReader));
        let csv = CsvUseCase::new(ext, Arc::new(FakeCsvReader));
        (classify, pdf, text_doc, spreadsheet, presentation, csv)
    }

    /// Build use cases whose classifier fails to classify (propagating the
    /// error through `classify`/`extract`).
    fn error_use_cases() -> (
        ClassifyDocumentUseCase,
        PdfUseCase,
        TextDocUseCase,
        SpreadsheetUseCase,
        PresentationUseCase,
        CsvUseCase,
    ) {
        use_cases_with_error(DocumentKind::Unknown)
    }

    fn use_cases_with_error(
        kind: DocumentKind,
    ) -> (
        ClassifyDocumentUseCase,
        PdfUseCase,
        TextDocUseCase,
        SpreadsheetUseCase,
        PresentationUseCase,
        CsvUseCase,
    ) {
        let ext = Arc::new(FakeClassifier {
            kind,
            error: Some(ExtractionError::Malformed("boom".into())),
        });
        let classify = ClassifyDocumentUseCase::new(ext.clone());
        let pdf = PdfUseCase::new(
            ext.clone(),
            Arc::new(FakePdfText),
            Arc::new(FakeRenderer),
            Arc::new(FakeOcr),
        );
        let text_doc = TextDocUseCase::new(ext.clone(), Arc::new(FakeTextReader));
        let spreadsheet = SpreadsheetUseCase::new(ext.clone(), Arc::new(FakeSheetReader));
        let presentation = PresentationUseCase::new(ext.clone(), Arc::new(FakePresReader));
        let csv = CsvUseCase::new(ext, Arc::new(FakeCsvReader));
        (classify, pdf, text_doc, spreadsheet, presentation, csv)
    }

    #[tokio::test]
    async fn run_cli_classify_prints_and_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Pdf);
        let cmd = CliCommand::Classify(PathBuf::from("a.pdf"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_pdf_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Pdf);
        let cmd = CliCommand::Extract(PathBuf::from("a.pdf"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_docx_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Docx);
        let cmd = CliCommand::Extract(PathBuf::from("a.docx"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_doc_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Doc);
        let cmd = CliCommand::Extract(PathBuf::from("a.doc"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_rtf_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Rtf);
        let cmd = CliCommand::Extract(PathBuf::from("a.rtf"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_xlsx_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Xlsx);
        let cmd = CliCommand::Extract(PathBuf::from("a.xlsx"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_xls_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Xls);
        let cmd = CliCommand::Extract(PathBuf::from("a.xls"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_pptx_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Pptx);
        let cmd = CliCommand::Extract(PathBuf::from("a.pptx"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_ppt_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Ppt);
        let cmd = CliCommand::Extract(PathBuf::from("a.ppt"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_csv_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Csv);
        let cmd = CliCommand::Extract(PathBuf::from("a.csv"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_extract_odt_fallback_ok() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Odt);
        let cmd = CliCommand::Extract(PathBuf::from("a.odt"));
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_ok());
    }

    #[tokio::test]
    async fn run_cli_classify_error_propagates() {
        let (c, pdf, td, ss, pr, cv) = error_use_cases();
        let cmd = CliCommand::Classify(PathBuf::from("a.pdf"));
        let err = run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv)
            .await
            .unwrap_err();
        assert!(err.contains("boom"));
    }

    #[tokio::test]
    async fn run_cli_extract_error_propagates() {
        let (c, pdf, td, ss, pr, cv) = error_use_cases();
        let cmd = CliCommand::Extract(PathBuf::from("a.pdf"));
        let err = run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv)
            .await
            .unwrap_err();
        assert!(err.contains("boom"));
    }

    #[tokio::test]
    async fn run_cli_serve_returns_error() {
        let (c, pdf, td, ss, pr, cv) = use_cases(DocumentKind::Pdf);
        let cmd = CliCommand::Serve;
        assert!(run_cli(&cmd, &c, &pdf, &td, &ss, &pr, &cv).await.is_err());
    }
}
