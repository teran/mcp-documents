//! Binary entrypoint.
//!
//! Dispatch: if argv contains a recognized subcommand (`classify`, `extract`,
//! `serve`) the corresponding CLI path runs; otherwise the MCP stdio server
//! runs.

use std::sync::Arc;

use mcp_documents::application::{
    ClassifyDocumentUseCase, CsvUseCase, PdfUseCase, PresentationUseCase, SpreadsheetUseCase,
    TextDocUseCase,
};
use mcp_documents::domain::ports::{
    DocumentExtractor, OcrProvider, PdfPageRenderer, PdfTextSource,
};
use mcp_documents::domain::result::Source;
use mcp_documents::infrastructure::{
    AnyDocExtractor, AnyDocPresentationReader, AnyDocTextReader, CalamineSpreadsheetReader,
    CsvStreamReader, PdfRenderer, PdfTextSourceAdapter,
};
use mcp_documents::interface::cli::{CliCommand, CliDispatch};
use mcp_documents::interface::{DocumentsServer, run_cli, serve_stdio};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Build the application use cases with the concrete adapters.
    let extractor: Arc<dyn DocumentExtractor> = Arc::new(AnyDocExtractor::new());
    let renderer: Arc<dyn PdfPageRenderer> = Arc::new(PdfRenderer::new());
    let ocr: Arc<dyn OcrProvider> = build_ocr()?;
    let pdf_text: Arc<dyn PdfTextSource> = Arc::new(PdfTextSourceAdapter::new());

    let classify = ClassifyDocumentUseCase::new(extractor.clone());
    let pdf = PdfUseCase::new(extractor.clone(), pdf_text, renderer, ocr);
    let text_doc = TextDocUseCase::new(extractor.clone(), Arc::new(AnyDocTextReader::new()));
    let spreadsheet = SpreadsheetUseCase::new(
        extractor.clone(),
        Arc::new(CalamineSpreadsheetReader::new()),
    );
    let presentation = PresentationUseCase::new(
        extractor.clone(),
        Arc::new(AnyDocPresentationReader::new(None)),
    );
    let csv = CsvUseCase::new(extractor.clone(), Arc::new(CsvStreamReader::new()));

    // CLI dispatch vs MCP stdio server.
    match CliCommand::dispatch(&args) {
        CliDispatch::Cli(cmd) => {
            // Enforce the same source gates as the MCP tools (ALLOW_ROOT_DIR +
            // MAX_INPUT_BYTES, with tilde expansion) — the CLI otherwise builds
            // `Source::Path` directly and would bypass them.
            validate_cli_path(&cmd)?;
            run_cli(
                &cmd,
                &classify,
                &pdf,
                &text_doc,
                &spreadsheet,
                &presentation,
                &csv,
            )
            .await
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
            Ok(())
        }
        CliDispatch::Server => {
            // `serve`, an unknown subcommand, or no arguments: run the MCP
            // stdio server.
            let server =
                DocumentsServer::new(classify, pdf, text_doc, spreadsheet, presentation, csv);
            serve_stdio(server).await
        }
    }
}

/// Enforce the shared source gates (`ALLOW_ROOT_DIR` + `MAX_INPUT_BYTES`, with
/// `~` expansion) on a CLI path before it is read, mirroring the MCP tools.
fn validate_cli_path(cmd: &CliCommand) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = match cmd {
        CliCommand::Classify(p) | CliCommand::Extract(p) => p,
        CliCommand::Serve => return Ok(()),
    };
    let expanded = mcp_documents::config::expand_tilde(&path.to_string_lossy());
    mcp_documents::infrastructure::file_reader::validate_source(&Source::Path(expanded))
        .map(|_| ())
        .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()))
}

/// Select the OCR provider based on the enabled feature.
///
/// - `ocr-native` enabled → the real Tesseract-backed provider.
/// - otherwise → a stub that reports OCR unavailable.
#[cfg(not(feature = "ocr-native"))]
fn build_ocr() -> Result<Arc<dyn OcrProvider>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(mcp_documents::infrastructure::make_ocr())
}

/// Select the OCR provider when `ocr-native` is enabled. Fails fast at startup
/// if the engine cannot be initialized.
#[cfg(feature = "ocr-native")]
fn build_ocr() -> Result<Arc<dyn OcrProvider>, Box<dyn std::error::Error + Send + Sync>> {
    mcp_documents::infrastructure::make_ocr().map_err(|e| {
        Box::<dyn std::error::Error + Send + Sync>::from(format!(
            "failed to initialize OCR engine: {e}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_documents::domain::result::PageBitmap;
    use std::path::PathBuf;

    /// With OCR disabled (default build), the provider built at startup is a
    /// stub that reports OCR unavailable rather than panicking.
    #[cfg(not(feature = "ocr-native"))]
    #[test]
    fn build_ocr_default_build_is_noop_provider() {
        let ocr = build_ocr().unwrap();
        let bmp = PageBitmap::new(1, 1, 1, vec![0]).unwrap();
        let err = ocr.ocr_page(&bmp).unwrap_err();
        assert!(matches!(
            err,
            mcp_documents::domain::error::ExtractionError::OcrUnavailable(_)
        ));
    }

    #[test]
    fn validate_cli_path_serve_returns_ok() {
        // `serve` has no path to gate — it must short-circuit to Ok.
        assert!(validate_cli_path(&CliCommand::Serve).is_ok());
    }

    #[test]
    fn validate_cli_path_rejects_missing_file() {
        // A `classify` path that cannot be resolved must fail the shared source
        // gate (ALLOW_ROOT_DIR + file existence).
        let missing = format!(
            "{}/definitely-missing-{}",
            std::env::temp_dir().display(),
            std::process::id()
        );
        let err = validate_cli_path(&CliCommand::Classify(PathBuf::from(&missing)));
        assert!(err.is_err());
    }

    #[test]
    fn validate_cli_path_accepts_existing_file_inside_root() {
        let dir = std::env::temp_dir().join(format!("mcp-main-v-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.txt");
        std::fs::write(&p, b"x").unwrap();
        unsafe { std::env::set_var("ALLOW_ROOT_DIR", &dir) };
        let res = validate_cli_path(&CliCommand::Extract(p.clone()));
        unsafe { std::env::remove_var("ALLOW_ROOT_DIR") };
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(res.is_ok());
    }

    #[test]
    fn validate_cli_path_valid_classify_path_ok() {
        let dir = std::env::temp_dir().join(format!("mcp-main-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        std::fs::write(&p, b"hi").unwrap();
        unsafe { std::env::set_var("ALLOW_ROOT_DIR", &dir) };
        assert!(validate_cli_path(&CliCommand::Classify(p.clone())).is_ok());
        unsafe { std::env::remove_var("ALLOW_ROOT_DIR") };
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
