//! Binary entrypoint.
//!
//! Dispatch: if argv contains a recognized subcommand (`classify`, `extract`,
//! `extract-text`, `serve`) the corresponding CLI path runs; otherwise the
//! MCP stdio server runs.

use std::sync::Arc;

use mcp_documents::application::{ClassifyDocumentUseCase, ExtractDocumentUseCase};
use mcp_documents::domain::ports::{DocumentExtractor, OcrProvider, PdfPageRenderer};
use mcp_documents::infrastructure::AnyDocExtractor;
use mcp_documents::infrastructure::PdfRenderer;
use mcp_documents::interface::cli::CliCommand;
use mcp_documents::interface::{DocumentsServer, run_cli, serve_stdio};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Build the application use cases with the concrete adapters.
    let extractor: Arc<dyn DocumentExtractor> = Arc::new(AnyDocExtractor::new());
    let renderer: Arc<dyn PdfPageRenderer> = Arc::new(PdfRenderer::new());
    let ocr: Arc<dyn OcrProvider> = build_ocr()?;

    let classify = ClassifyDocumentUseCase::new(extractor.clone());
    let extract = ExtractDocumentUseCase::new(extractor, ocr, renderer);

    // CLI dispatch vs MCP stdio server.
    match CliCommand::parse(&args) {
        Some(cmd) if !cmd.is_serve() => {
            run_cli(&cmd, &classify, &extract)
                .await
                .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
            Ok(())
        }
        _ => {
            // No recognized subcommand, or `serve`: run the MCP stdio server.
            let server = DocumentsServer::new(classify, extract);
            serve_stdio(server).await
        }
    }
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
