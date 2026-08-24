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
}
