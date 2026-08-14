//! The MCP server: a `rmcp`-based tool router wrapping the application use
//! cases, plus the stdio entrypoint.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

use crate::application::{ClassifyDocumentUseCase, ExtractDocumentUseCase};
use crate::domain::result::Source;

use super::tools::{ClassifyParams, ExtractParams, json_result, text_result, tool_error};

/// The MCP server exposing document tools over stdio.
#[derive(Clone)]
pub struct DocumentsServer {
    classify: ClassifyDocumentUseCase,
    extract: ExtractDocumentUseCase,
    tool_router: ToolRouter<Self>,
}

impl DocumentsServer {
    /// Build the server around its use cases.
    pub fn new(classify: ClassifyDocumentUseCase, extract: ExtractDocumentUseCase) -> Self {
        Self {
            classify,
            extract,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
impl DocumentsServer {
    /// Classify a PDF, returning its type and the pages needing OCR.
    #[tool(description = "Classify a PDF, returning its type and pages needing OCR")]
    async fn classify_pdf(
        &self,
        Parameters(params): Parameters<ClassifyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let source = Source::Path(std::path::PathBuf::from(params.path));
        match self.classify.classify(&source) {
            Ok(c) => Ok(json_result(rmcp::serde_json::json!({
                "kind": c.kind.as_str(),
                "pdf_type": c.pdf_type.map(|t| format!("{t:?}")),
                "pages_needing_ocr": c.pages_needing_ocr
                    .iter()
                    .map(|p| rmcp::serde_json::json!({
                        "page_index": p.page_index,
                        "needs_ocr": p.needs_ocr,
                        "confidence": p.confidence,
                    }))
                    .collect::<Vec<_>>(),
            }))),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Extract a PDF to Markdown (local extraction + OCR for scanned pages).
    #[tool(description = "Extract Markdown text from a PDF (with OCR for scanned pages)")]
    async fn extract_pdf(
        &self,
        Parameters(params): Parameters<ExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let source = Source::Path(std::path::PathBuf::from(params.path));
        match self.extract.extract(&source).await {
            Ok(result) => Ok(text_result(result.markdown)),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Extract a PDF to plain text (local extraction + OCR for scanned pages).
    #[tool(description = "Extract plain text from a PDF (with OCR for scanned pages)")]
    async fn extract_pdf_text(
        &self,
        Parameters(params): Parameters<ExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let source = Source::Path(std::path::PathBuf::from(params.path));
        match self.extract.extract(&source).await {
            Ok(result) => Ok(text_result(result.markdown)),
            Err(e) => Ok(tool_error(&e)),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DocumentsServer {}

/// Serve the MCP server over stdio until the client disconnects.
pub async fn serve_stdio(
    server: DocumentsServer,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use rmcp::ServiceExt;
    use rmcp::transport::stdio;

    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
