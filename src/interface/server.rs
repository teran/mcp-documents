//! The MCP server: a `rmcp`-based tool router wrapping the application use
//! cases, plus the stdio entrypoint.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

use crate::application::{ClassifyDocumentUseCase, ExtractDocumentUseCase};

use super::tools::{
    source_from_params, ClassifyParams, ExtractParams, json_result, text_result, tool_error,
};

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
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
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
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
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
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        match self.extract.extract(&source).await {
            Ok(result) => Ok(text_result(result.markdown)),
            Err(e) => Ok(tool_error(&e)),
        }
    }
}

#[tool_handler(
    router = self.tool_router,
    instructions = "You cannot read PDF or Office document files directly — the model does not support binary/PDF input. When the user asks to extract text from a PDF, ALWAYS call one of this server's tools with the file `path` instead of opening or parsing the file yourself: `classify_pdf` (detects type + pages needing OCR), `extract_pdf` (Markdown), or `extract_pdf_text` (plain text). Never attempt to read the raw attachment or file bytes."
)]
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::application::{ClassifyDocumentUseCase, ExtractDocumentUseCase};
    use crate::domain::document::{Classification, DocumentKind, PdfType};
    use crate::domain::error::ExtractionError;
    use crate::domain::ports::{DocumentExtractor, OcrProvider, PdfPageRenderer};
    use crate::domain::result::{ExtractionResult, PageBitmap, Source};
    use rmcp::handler::server::wrapper::Parameters;

    use super::*;
    use crate::interface::tools::{ClassifyParams, ExtractParams};

    /// Fake extractor that classifies every input as a text-based PDF and
    /// returns a canned Markdown body on extraction.
    #[derive(Clone)]
    struct FakeExtractor {
        fail_classify: bool,
        fail_extract: bool,
    }

    impl DocumentExtractor for FakeExtractor {
        fn classify(&self, _source: &Source) -> Result<Classification, ExtractionError> {
            if self.fail_classify {
                return Err(ExtractionError::Malformed("bad classify".into()));
            }
            Ok(Classification {
                kind: DocumentKind::Pdf,
                pdf_type: Some(PdfType::TextBased),
                pages_needing_ocr: Vec::new(),
            })
        }

        fn extract_text(&self, _source: &Source) -> Result<ExtractionResult, ExtractionError> {
            if self.fail_extract {
                return Err(ExtractionError::Unsupported(DocumentKind::Pdf));
            }
            Ok(ExtractionResult::markdown_only(
                DocumentKind::Pdf,
                Some(PdfType::TextBased),
                "extracted markdown".into(),
            ))
        }
    }

    struct FakeOcr;
    impl OcrProvider for FakeOcr {
        fn ocr_page(&self, _bitmap: &PageBitmap) -> Result<String, ExtractionError> {
            Err(ExtractionError::OcrUnavailable("no".into()))
        }
    }

    struct FakeRenderer;
    impl PdfPageRenderer for FakeRenderer {
        fn render_page(
            &self,
            _pdf: &[u8],
            _idx: usize,
            _dpi: u32,
        ) -> Result<PageBitmap, ExtractionError> {
            Err(ExtractionError::Internal("no render".into()))
        }
    }

    fn server(fail_classify: bool, fail_extract: bool) -> DocumentsServer {
        let extractor = Arc::new(FakeExtractor {
            fail_classify,
            fail_extract,
        });
        let classify = ClassifyDocumentUseCase::new(extractor.clone());
        let extract =
            ExtractDocumentUseCase::new(extractor, Arc::new(FakeOcr), Arc::new(FakeRenderer));
        DocumentsServer::new(classify, extract)
    }

    #[tokio::test]
    async fn classify_pdf_success_returns_json() {
        let s = server(false, false);
        let res = s
            .classify_pdf(Parameters(ClassifyParams {
                path: "whatever.pdf".into(),
                bytes: None,
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(false));
        let text = res.content[0].as_text().unwrap().text.clone();
        assert!(text.contains("\"kind\": \"pdf\""));
        assert!(text.contains("\"pdf_type\": \"TextBased\""));
    }

    #[tokio::test]
    async fn classify_pdf_error_is_tool_error() {
        let s = server(true, false);
        let res = s
            .classify_pdf(Parameters(ClassifyParams {
                path: "bad.pdf".into(),
                bytes: None,
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(true));
        assert!(res.content[0].as_text().unwrap().text.contains("malformed"));
    }

    #[tokio::test]
    async fn extract_pdf_success_returns_text() {
        let s = server(false, false);
        let res = s
            .extract_pdf(Parameters(ExtractParams {
                path: "a.pdf".into(),
                bytes: None,
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(false));
        assert_eq!(res.content[0].as_text().unwrap().text, "extracted markdown");
    }

    #[tokio::test]
    async fn extract_pdf_text_success_returns_text() {
        let s = server(false, false);
        let res = s
            .extract_pdf_text(Parameters(ExtractParams {
                path: "a.pdf".into(),
                bytes: None,
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(false));
        assert_eq!(res.content[0].as_text().unwrap().text, "extracted markdown");
    }

    #[tokio::test]
    async fn extract_pdf_error_is_tool_error() {
        let s = server(false, true);
        let res = s
            .extract_pdf(Parameters(ExtractParams {
                path: "bad.pdf".into(),
                bytes: None,
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(true));
        assert!(
            res.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("unsupported")
        );
    }
}
