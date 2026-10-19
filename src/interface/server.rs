//! The MCP server: a `rmcp`-based tool router wrapping the application use
//! cases, plus the stdio entrypoint.
//!
//! Tool surface is per-format: `<input_format>_<action>` where action is `info`
//! or `extract_markdown`, plus the single generic `classify_document`. Markdown
//! is the only output; there are no plain-text tools.

use std::ops::Range;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

use crate::application::{
    ClassifyDocumentUseCase, CsvUseCase, PdfUseCase, PresentationUseCase, SpreadsheetUseCase,
    TextDocUseCase,
};
use crate::domain::document::DocumentKind;

use super::tools::{
    ClassifyParams, CsvExtractParams, CsvInfoParams, PdfExtractParams, PdfInfoParams,
    PresentationExtractParams, PresentationInfoParams, SpreadsheetExtractParams,
    SpreadsheetInfoParams, TextExtractParams, TextInfoParams, json_result, source_from_params,
    text_result, tool_error,
};

/// The MCP server exposing document tools over stdio.
#[derive(Clone)]
pub struct DocumentsServer {
    classify: ClassifyDocumentUseCase,
    pdf: PdfUseCase,
    text_doc: TextDocUseCase,
    spreadsheet: SpreadsheetUseCase,
    presentation: PresentationUseCase,
    csv: CsvUseCase,
    tool_router: ToolRouter<Self>,
}

impl DocumentsServer {
    /// Build the server around its use cases.
    pub fn new(
        classify: ClassifyDocumentUseCase,
        pdf: PdfUseCase,
        text_doc: TextDocUseCase,
        spreadsheet: SpreadsheetUseCase,
        presentation: PresentationUseCase,
        csv: CsvUseCase,
    ) -> Self {
        Self {
            classify,
            pdf,
            text_doc,
            spreadsheet,
            presentation,
            csv,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
impl DocumentsServer {
    // ------------------------------------------------------------------
    // classify_document (the ONLY generic classifier)
    // ------------------------------------------------------------------

    /// Detect the format/kind of an unknown file (path or bytes). For a PDF
    /// also returns the PDF type (TextBased/Scanned/ImageBased/Mixed) and the
    /// pages needing OCR.
    #[tool(
        description = "Classify any supported document, returning its kind and (for PDFs) the PDF type and pages needing OCR. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; the tool never modifies its environment.",
        annotations(
            title = "Classify document",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn classify_document(
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
                "pdf_type": c.pdf_type.map(|t| t.as_str()),
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

    // ------------------------------------------------------------------
    // PDF
    // ------------------------------------------------------------------

    /// PDF metadata: page count, PDF type (TextBased/Scanned/ImageBased/Mixed)
    /// and which pages need OCR.
    #[tool(
        description = "PDF metadata: page count, PDF type, and pages needing OCR. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = "PDF metadata",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn pdf_info(
        &self,
        Parameters(params): Parameters<PdfInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        match self.pdf.info(&source) {
            Ok(info) => Ok(json_result(rmcp::serde_json::json!({
                "kind": "pdf",
                "page_count": info.page_count,
                "pdf_type": info.pdf_type.as_str(),
                "pages_needing_ocr": info.pages_needing_ocr
                    .iter()
                    .map(|p| rmcp::serde_json::json!({
                        "page_index": p.page_index,
                        "needs_ocr": p.needs_ocr,
                    }))
                    .collect::<Vec<_>>(),
            }))),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Extract a PDF as Markdown. Scanned pages are OCR'd; the optional
    /// `page_from`/`page_to` (0-based, end-exclusive) selects a page range.
    #[tool(
        description = "Extract a PDF to Markdown (OCR for scanned pages); optional 0-based page_from/page_to range. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract PDF as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn pdf_extract_markdown(
        &self,
        Parameters(params): Parameters<PdfExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        let range = to_range(params.page_from, params.page_to);
        match self.pdf.extract_markdown(&source, range).await {
            Ok(result) => Ok(text_result(result.markdown)),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    // ------------------------------------------------------------------
    // Word-processor formats (doc / docx / rtf) — word & char counts
    // ------------------------------------------------------------------

    /// Word count and character count for a legacy binary `.doc`.
    #[tool(
        description = "Word and character counts for a .doc document. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = ".doc word counts",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn doc_info(
        &self,
        Parameters(params): Parameters<TextInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.text_info(&params, DocumentKind::Doc).await
    }

    /// Extract a `.doc` as Markdown; optional `word_from`/`word_to` (0-based,
    /// end-exclusive) selects a word range.
    #[tool(
        description = "Extract a .doc to Markdown; optional 0-based word_from/word_to word range. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract .doc as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn doc_extract_markdown(
        &self,
        Parameters(params): Parameters<TextExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.text_extract(&params, DocumentKind::Doc).await
    }

    /// Word count and character count for a `.docx`.
    #[tool(
        description = "Word and character counts for a .docx document. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = ".docx word counts",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn docx_info(
        &self,
        Parameters(params): Parameters<TextInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.text_info(&params, DocumentKind::Docx).await
    }

    /// Extract a `.docx` as Markdown; optional `word_from`/`word_to` (0-based,
    /// end-exclusive) selects a word range.
    #[tool(
        description = "Extract a .docx to Markdown; optional 0-based word_from/word_to word range. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract .docx as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn docx_extract_markdown(
        &self,
        Parameters(params): Parameters<TextExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.text_extract(&params, DocumentKind::Docx).await
    }

    /// Word count and character count for an RTF document.
    #[tool(
        description = "Word and character counts for an RTF document. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = "RTF word counts",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn rtf_info(
        &self,
        Parameters(params): Parameters<TextInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.text_info(&params, DocumentKind::Rtf).await
    }

    /// Extract an RTF document as Markdown; optional `word_from`/`word_to`
    /// (0-based, end-exclusive) selects a word range.
    #[tool(
        description = "Extract an RTF document to Markdown; optional 0-based word_from/word_to word range. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract RTF as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn rtf_extract_markdown(
        &self,
        Parameters(params): Parameters<TextExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.text_extract(&params, DocumentKind::Rtf).await
    }

    // ------------------------------------------------------------------
    // Spreadsheets (xls / xlsx) — sheets + rows
    // ------------------------------------------------------------------

    /// List the sheets (with row/col counts) of a legacy binary `.xls`.
    #[tool(
        description = "List the sheets of a .xls workbook. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = ".xls sheet list",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn xls_info(
        &self,
        Parameters(params): Parameters<SpreadsheetInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.spreadsheet_info(&params, DocumentKind::Xls).await
    }

    /// Extract a `.xls` sheet as a Markdown table; optional `sheet` name and
    /// `row_from`/`row_to` (0-based, end-exclusive) row range.
    #[tool(
        description = "Extract a .xls sheet as a Markdown table; optional sheet name and 0-based row_from/row_to range. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract .xls sheet as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn xls_extract_markdown(
        &self,
        Parameters(params): Parameters<SpreadsheetExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.spreadsheet_extract(&params, DocumentKind::Xls).await
    }

    /// List the sheets (with row/col counts) of a `.xlsx` workbook.
    #[tool(
        description = "List the sheets of a .xlsx workbook. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = ".xlsx sheet list",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn xlsx_info(
        &self,
        Parameters(params): Parameters<SpreadsheetInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.spreadsheet_info(&params, DocumentKind::Xlsx).await
    }

    /// Extract a `.xlsx` sheet as a Markdown table; optional `sheet` name and
    /// `row_from`/`row_to` (0-based, end-exclusive) row range.
    #[tool(
        description = "Extract a .xlsx sheet as a Markdown table; optional sheet name and 0-based row_from/row_to range. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract .xlsx sheet as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn xlsx_extract_markdown(
        &self,
        Parameters(params): Parameters<SpreadsheetExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.spreadsheet_extract(&params, DocumentKind::Xlsx).await
    }

    // ------------------------------------------------------------------
    // Presentations (ppt / pptx) — slides
    // ------------------------------------------------------------------

    /// Slide count for a legacy binary `.ppt` (whole-document unit; no range).
    #[tool(
        description = "Slide count for a .ppt presentation (legacy; whole-document unit). Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = ".ppt slide count",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn ppt_info(
        &self,
        Parameters(params): Parameters<PresentationInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.presentation_info(&params, DocumentKind::Ppt).await
    }

    /// Extract a `.ppt` as Markdown (whole document; `slide_from`/`slide_to`
    /// are not supported for legacy binary `.ppt`).
    #[tool(
        description = "Extract a .ppt to Markdown (whole document; no slide range for legacy binary .ppt). Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract .ppt as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn ppt_extract_markdown(
        &self,
        Parameters(params): Parameters<PresentationExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.presentation_extract(&params, DocumentKind::Ppt).await
    }

    /// Slide count for a `.pptx` presentation.
    #[tool(
        description = "Slide count for a .pptx presentation. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = ".pptx slide count",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn pptx_info(
        &self,
        Parameters(params): Parameters<PresentationInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.presentation_info(&params, DocumentKind::Pptx).await
    }

    /// Extract a `.pptx` as Markdown; optional `slide_from`/`slide_to`
    /// (0-based, end-exclusive) selects a slide range.
    #[tool(
        description = "Extract a .pptx to Markdown; optional 0-based slide_from/slide_to slide range. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract .pptx as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn pptx_extract_markdown(
        &self,
        Parameters(params): Parameters<PresentationExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.presentation_extract(&params, DocumentKind::Pptx).await
    }

    // ------------------------------------------------------------------
    // CSV (streaming)
    // ------------------------------------------------------------------

    /// CSV metadata in a single streaming pass: total rows, column count and
    /// header.
    #[tool(
        description = "CSV metadata: total rows, column count, header (streaming). Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; no side effects.",
        annotations(
            title = "CSV metadata",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn csv_info(
        &self,
        Parameters(params): Parameters<CsvInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        match self.csv.info(&source) {
            Ok(info) => Ok(json_result(rmcp::serde_json::json!({
                "kind": "csv",
                "row_count": info.row_count,
                "column_count": info.column_count,
                "header": info.header,
            }))),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Extract a CSV as a Markdown table (streaming); optional
    /// `row_from`/`row_to` (0-based over data rows, end-exclusive) selects a
    /// row range. The header row is always shown.
    #[tool(
        description = "Extract a CSV as a Markdown table (streaming); optional 0-based row_from/row_to over data rows. Read-only: pass a file `path` within ALLOW_ROOT_DIR or base64 `bytes`; output is Markdown only; no side effects.",
        annotations(
            title = "Extract CSV as Markdown",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn csv_extract_markdown(
        &self,
        Parameters(params): Parameters<CsvExtractParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        let range = to_range(params.row_from, params.row_to);
        match self.csv.extract_markdown(&source, range) {
            Ok(result) => Ok(text_result(result.markdown)),
            Err(e) => Ok(tool_error(&e)),
        }
    }
}

impl DocumentsServer {
    /// Shared helper for the word-processor `*_info` tools.
    async fn text_info(
        &self,
        params: &TextInfoParams,
        kind: DocumentKind,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        match self.text_doc.info(&source, kind) {
            Ok(info) => Ok(json_result(rmcp::serde_json::json!({
                "kind": kind.as_str(),
                "word_count": info.word_count,
                "char_count": info.char_count,
            }))),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Shared helper for the word-processor `*_extract_markdown` tools.
    async fn text_extract(
        &self,
        params: &TextExtractParams,
        kind: DocumentKind,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        let range = to_range(params.word_from, params.word_to);
        match self.text_doc.extract_markdown(&source, kind, range) {
            Ok(result) => Ok(text_result(result.markdown)),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Shared helper for the spreadsheet `*_info` tools.
    async fn spreadsheet_info(
        &self,
        params: &SpreadsheetInfoParams,
        kind: DocumentKind,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        match self.spreadsheet.info(&source, kind) {
            Ok(info) => Ok(json_result(rmcp::serde_json::json!({
                "kind": kind.as_str(),
                "sheets": info.sheets.iter().map(|s| rmcp::serde_json::json!({
                    "name": s.name,
                    "rows": s.rows,
                    "cols": s.cols,
                })).collect::<Vec<_>>(),
            }))),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Shared helper for the spreadsheet `*_extract_markdown` tools.
    async fn spreadsheet_extract(
        &self,
        params: &SpreadsheetExtractParams,
        kind: DocumentKind,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        let range = to_range(params.row_from, params.row_to);
        match self
            .spreadsheet
            .extract_markdown(&source, kind, params.sheet.as_deref(), range)
        {
            Ok(result) => Ok(text_result(result.markdown)),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Shared helper for the presentation `*_info` tools.
    async fn presentation_info(
        &self,
        params: &PresentationInfoParams,
        kind: DocumentKind,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        match self.presentation.info(&source, kind) {
            Ok(info) => Ok(json_result(rmcp::serde_json::json!({
                "kind": kind.as_str(),
                "slide_count": info.slide_count,
            }))),
            Err(e) => Ok(tool_error(&e)),
        }
    }

    /// Shared helper for the presentation `*_extract_markdown` tools.
    async fn presentation_extract(
        &self,
        params: &PresentationExtractParams,
        kind: DocumentKind,
    ) -> Result<CallToolResult, ErrorData> {
        let source = match source_from_params(&params.path, params.bytes.as_deref()) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(&e)),
        };
        let range = to_range(params.slide_from, params.slide_to);
        match self.presentation.extract_markdown(&source, kind, range) {
            Ok(result) => Ok(text_result(result.markdown)),
            Err(e) => Ok(tool_error(&e)),
        }
    }
}

/// Build an `Option<Range<usize>>` from optional from/to bounds.
fn to_range(from: Option<usize>, to: Option<usize>) -> Option<Range<usize>> {
    if from.is_none() && to.is_none() {
        None
    } else {
        Some(Range {
            start: from.unwrap_or(0),
            end: to.unwrap_or(usize::MAX),
        })
    }
}

#[tool_handler(
    router = self.tool_router,
    instructions = "You cannot read PDF or Office document files directly — the model does not support binary/PDF input. When the user asks to read a document, ALWAYS call one of this server's tools with the file `path` instead of opening or parsing the file yourself. Use `classify_document` to detect the format of an unknown file, then the matching `<format>_info` / `<format>_extract_markdown` tool: `pdf_info`/`pdf_extract_markdown`, `doc_info`/`doc_extract_markdown`, `docx_info`/`docx_extract_markdown`, `rtf_info`/`rtf_extract_markdown`, `xls_info`/`xls_extract_markdown`, `xlsx_info`/`xlsx_extract_markdown`, `ppt_info`/`ppt_extract_markdown`, `pptx_info`/`pptx_extract_markdown`, `csv_info`/`csv_extract_markdown`. Output is always Markdown. Never attempt to read the raw attachment or file bytes."
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

    use crate::application::{
        ClassifyDocumentUseCase, CsvUseCase, PdfUseCase, PresentationUseCase, SpreadsheetUseCase,
        TextDocUseCase,
    };
    use crate::domain::document::{Classification, DocumentKind, PdfType};
    use crate::domain::error::ExtractionError;
    use crate::domain::meta::{
        CsvInfo, PdfInfo, PresentationInfo, SpreadsheetInfo, TextDocumentInfo,
    };
    use crate::domain::ports::{
        CsvReader, DocumentExtractor, OcrProvider, PdfPageRenderer, PdfTextSource,
        PresentationReader, SpreadsheetReader, TextDocumentReader,
    };
    use crate::domain::result::{ExtractionResult, PageBitmap, Source};
    use rmcp::handler::server::wrapper::Parameters;

    use super::*;
    use crate::interface::tools::{
        ClassifyParams, CsvExtractParams, CsvInfoParams, PdfExtractParams, PdfInfoParams,
        TextInfoParams,
    };

    use base64::Engine as _;
    fn b64(payload: &[u8]) -> Option<String> {
        Some(base64::engine::general_purpose::STANDARD.encode(payload))
    }

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
            Ok(ExtractionResult::markdown_only(
                self.kind,
                None,
                "fallback".into(),
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
                page_count: 2,
                pdf_type: PdfType::TextBased,
                pages_needing_ocr: Vec::new(),
            })
        }
        fn extract_pages(
            &self,
            _s: &Source,
            _p: &[usize],
        ) -> Result<Vec<crate::domain::result::ExtractedPage>, ExtractionError> {
            Ok(_p
                .iter()
                .map(|&p| crate::domain::result::ExtractedPage::local(p, format!("text-{p}")))
                .collect())
        }
    }
    struct FakeTextReader;
    impl TextDocumentReader for FakeTextReader {
        fn text_info(&self, _s: &Source) -> Result<TextDocumentInfo, ExtractionError> {
            Ok(TextDocumentInfo {
                word_count: 3,
                char_count: 10,
            })
        }
        fn extract_text(&self, _s: &Source) -> Result<String, ExtractionError> {
            Ok("one two three".into())
        }
    }
    struct FakeSheet;
    impl SpreadsheetReader for FakeSheet {
        fn spreadsheet_info(&self, _s: &Source) -> Result<SpreadsheetInfo, ExtractionError> {
            Ok(SpreadsheetInfo { sheets: vec![] })
        }
        fn extract_sheet(
            &self,
            _s: &Source,
            _sh: Option<&str>,
            _r: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("table".into())
        }
    }
    struct FakePres;
    impl PresentationReader for FakePres {
        fn presentation_info(&self, _s: &Source) -> Result<PresentationInfo, ExtractionError> {
            Ok(PresentationInfo { slide_count: 1 })
        }
        fn extract_presentation(
            &self,
            _s: &Source,
            _r: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("slide".into())
        }
    }
    struct FakeCsv;
    impl CsvReader for FakeCsv {
        fn csv_info(&self, _s: &Source) -> Result<CsvInfo, ExtractionError> {
            Ok(CsvInfo {
                row_count: 1,
                column_count: 2,
                header: vec!["a".into(), "b".into()],
            })
        }
        fn extract_csv(
            &self,
            _s: &Source,
            _r: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("csv".into())
        }
    }

    fn server(kind: DocumentKind) -> DocumentsServer {
        let ext = Arc::new(FakeClassifier { kind });
        let classify = ClassifyDocumentUseCase::new(ext.clone());
        let pdf = PdfUseCase::new(
            ext.clone(),
            Arc::new(FakePdfText),
            Arc::new(FakeRenderer),
            Arc::new(FakeOcr),
        );
        let text_doc = TextDocUseCase::new(ext.clone(), Arc::new(FakeTextReader));
        let spreadsheet = SpreadsheetUseCase::new(ext.clone(), Arc::new(FakeSheet));
        let presentation = PresentationUseCase::new(ext.clone(), Arc::new(FakePres));
        let csv = CsvUseCase::new(ext, Arc::new(FakeCsv));
        DocumentsServer::new(classify, pdf, text_doc, spreadsheet, presentation, csv)
    }

    #[tokio::test]
    async fn classify_document_returns_json() {
        let s = server(DocumentKind::Docx);
        let res = s
            .classify_document(Parameters(ClassifyParams {
                path: "x.docx".into(),
                bytes: b64(b"x"),
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(false));
        let text = res.content[0].as_text().unwrap().text.clone();
        assert!(text.contains("\"kind\": \"docx\""));
    }

    #[tokio::test]
    async fn pdf_info_success_returns_json() {
        let s = server(DocumentKind::Pdf);
        let res = s
            .pdf_info(Parameters(PdfInfoParams {
                path: "x.pdf".into(),
                bytes: b64(b"x"),
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(false));
        let text = res.content[0].as_text().unwrap().text.clone();
        assert!(text.contains("\"page_count\": 2"));
    }

    #[tokio::test]
    async fn docx_info_rejects_non_docx_as_tool_error() {
        // Classifier says Pdf, but docx_info requires Docx -> Unsupported.
        let s = server(DocumentKind::Pdf);
        let res = s
            .docx_info(Parameters(TextInfoParams {
                path: "x.docx".into(),
                bytes: b64(b"x"),
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

    #[tokio::test]
    async fn pdf_extract_markdown_returns_text() {
        let s = server(DocumentKind::Pdf);
        let res = s
            .pdf_extract_markdown(Parameters(PdfExtractParams {
                path: "x.pdf".into(),
                bytes: b64(b"x"),
                page_from: None,
                page_to: None,
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(false));
        assert_eq!(res.content[0].as_text().unwrap().text, "text-0\n\ntext-1");
    }

    #[tokio::test]
    async fn csv_extract_markdown_returns_text() {
        let s = server(DocumentKind::Csv);
        let res = s
            .csv_extract_markdown(Parameters(CsvExtractParams {
                path: "x.csv".into(),
                bytes: b64(b"x"),
                row_from: None,
                row_to: None,
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(false));
        assert_eq!(res.content[0].as_text().unwrap().text, "csv");
    }

    #[tokio::test]
    async fn csv_info_on_non_csv_is_tool_error() {
        let s = server(DocumentKind::Pdf);
        let res = s
            .csv_info(Parameters(CsvInfoParams {
                path: "x.csv".into(),
                bytes: b64(b"x"),
            }))
            .await
            .unwrap();
        assert_eq!(res.is_error, Some(true));
    }
}

#[cfg(test)]
mod extra_tests {
    use std::sync::Arc;

    use crate::application::{
        ClassifyDocumentUseCase, CsvUseCase, PdfUseCase, PresentationUseCase, SpreadsheetUseCase,
        TextDocUseCase,
    };
    use crate::domain::document::{Classification, DocumentKind, PdfType};
    use crate::domain::error::ExtractionError;
    use crate::domain::meta::{
        CsvInfo, PdfInfo, PresentationInfo, SpreadsheetInfo, TextDocumentInfo,
    };
    use crate::domain::ports::{
        CsvReader, DocumentExtractor, OcrProvider, PdfPageRenderer, PdfTextSource,
        PresentationReader, SpreadsheetReader, TextDocumentReader,
    };
    use crate::domain::result::{ExtractionResult, PageBitmap, Source};
    use base64::Engine as _;
    use rmcp::handler::server::wrapper::Parameters;

    use super::*;
    use crate::interface::tools::{
        PdfInfoParams, PresentationExtractParams, PresentationInfoParams, SpreadsheetExtractParams,
        SpreadsheetInfoParams, TextExtractParams, TextInfoParams,
    };

    fn b64(payload: &[u8]) -> Option<String> {
        Some(base64::engine::general_purpose::STANDARD.encode(payload))
    }

    #[derive(Clone)]
    struct Cls {
        kind: DocumentKind,
    }
    impl DocumentExtractor for Cls {
        fn classify(&self, _s: &Source) -> Result<Classification, ExtractionError> {
            let pt = if self.kind == DocumentKind::Pdf {
                Some(PdfType::TextBased)
            } else {
                None
            };
            Ok(Classification {
                kind: self.kind,
                pdf_type: pt,
                pages_needing_ocr: Vec::new(),
            })
        }
        fn extract_text(&self, _s: &Source) -> Result<ExtractionResult, ExtractionError> {
            Ok(ExtractionResult::markdown_only(self.kind, None, "f".into()))
        }
    }
    struct O;
    impl OcrProvider for O {
        fn ocr_page(&self, _b: &PageBitmap) -> Result<String, ExtractionError> {
            Err(ExtractionError::OcrUnavailable("n".into()))
        }
    }
    struct R;
    impl PdfPageRenderer for R {
        fn render_page(
            &self,
            _p: &[u8],
            _i: usize,
            _d: u32,
        ) -> Result<PageBitmap, ExtractionError> {
            Err(ExtractionError::Internal("r".into()))
        }
    }
    struct PT;
    impl PdfTextSource for PT {
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
            unreachable!()
        }
    }
    struct TR;
    impl TextDocumentReader for TR {
        fn text_info(&self, _s: &Source) -> Result<TextDocumentInfo, ExtractionError> {
            Ok(TextDocumentInfo {
                word_count: 1,
                char_count: 4,
            })
        }
        fn extract_text(&self, _s: &Source) -> Result<String, ExtractionError> {
            Ok("md".into())
        }
    }
    struct SR;
    impl SpreadsheetReader for SR {
        fn spreadsheet_info(&self, _s: &Source) -> Result<SpreadsheetInfo, ExtractionError> {
            Ok(SpreadsheetInfo {
                sheets: vec![crate::domain::meta::SheetInfo {
                    name: "S1".into(),
                    rows: Some(2),
                    cols: Some(1),
                }],
            })
        }
        fn extract_sheet(
            &self,
            _s: &Source,
            _sh: Option<&str>,
            _r: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("tbl".into())
        }
    }
    struct PR;
    impl PresentationReader for PR {
        fn presentation_info(&self, _s: &Source) -> Result<PresentationInfo, ExtractionError> {
            Ok(PresentationInfo { slide_count: 2 })
        }
        fn extract_presentation(
            &self,
            _s: &Source,
            _r: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("sld".into())
        }
    }
    struct CR;
    impl CsvReader for CR {
        fn csv_info(&self, _s: &Source) -> Result<CsvInfo, ExtractionError> {
            Ok(CsvInfo {
                row_count: 1,
                column_count: 2,
                header: vec!["a".into(), "b".into()],
            })
        }
        fn extract_csv(
            &self,
            _s: &Source,
            _r: Option<Range<usize>>,
        ) -> Result<String, ExtractionError> {
            Ok("csv".into())
        }
    }

    fn svc(kind: DocumentKind) -> DocumentsServer {
        let e = Arc::new(Cls { kind });
        let classify = ClassifyDocumentUseCase::new(e.clone());
        let pdf = PdfUseCase::new(e.clone(), Arc::new(PT), Arc::new(R), Arc::new(O));
        let td = TextDocUseCase::new(e.clone(), Arc::new(TR));
        let ss = SpreadsheetUseCase::new(e.clone(), Arc::new(SR));
        let pr = PresentationUseCase::new(e.clone(), Arc::new(PR));
        let cv = CsvUseCase::new(e, Arc::new(CR));
        DocumentsServer::new(classify, pdf, td, ss, pr, cv)
    }

    #[tokio::test]
    async fn text_info_and_extract_tools() {
        for kind in [DocumentKind::Doc, DocumentKind::Docx, DocumentKind::Rtf] {
            let s = svc(kind);
            let info = s
                .text_info(
                    &TextInfoParams {
                        path: "x".into(),
                        bytes: b64(b"x"),
                    },
                    kind,
                )
                .await
                .unwrap();
            assert_eq!(info.is_error, Some(false));
            let ext = s
                .text_extract(
                    &TextExtractParams {
                        path: "x".into(),
                        bytes: b64(b"x"),
                        word_from: None,
                        word_to: None,
                    },
                    kind,
                )
                .await
                .unwrap();
            assert_eq!(ext.is_error, Some(false));
        }
    }

    #[tokio::test]
    async fn spreadsheet_info_and_extract_tools() {
        for kind in [DocumentKind::Xls, DocumentKind::Xlsx] {
            let s = svc(kind);
            let info = s
                .spreadsheet_info(
                    &SpreadsheetInfoParams {
                        path: "x".into(),
                        bytes: b64(b"x"),
                    },
                    kind,
                )
                .await
                .unwrap();
            assert_eq!(info.is_error, Some(false));
            let ext = s
                .spreadsheet_extract(
                    &SpreadsheetExtractParams {
                        path: "x".into(),
                        bytes: b64(b"x"),
                        sheet: Some("S1".into()),
                        row_from: None,
                        row_to: None,
                    },
                    kind,
                )
                .await
                .unwrap();
            assert_eq!(ext.is_error, Some(false));
        }
    }

    #[tokio::test]
    async fn presentation_info_and_extract_tools() {
        for kind in [DocumentKind::Ppt, DocumentKind::Pptx] {
            let s = svc(kind);
            let info = s
                .presentation_info(
                    &PresentationInfoParams {
                        path: "x".into(),
                        bytes: b64(b"x"),
                    },
                    kind,
                )
                .await
                .unwrap();
            assert_eq!(info.is_error, Some(false));
            let ext = s
                .presentation_extract(
                    &PresentationExtractParams {
                        path: "x".into(),
                        bytes: b64(b"x"),
                        slide_from: None,
                        slide_to: None,
                    },
                    kind,
                )
                .await
                .unwrap();
            assert_eq!(ext.is_error, Some(false));
        }
    }

    #[tokio::test]
    async fn direct_tool_methods_round_trip() {
        let s = svc(DocumentKind::Docx);
        assert_eq!(
            s.docx_info(Parameters(TextInfoParams {
                path: "x".into(),
                bytes: b64(b"x")
            }))
            .await
            .unwrap()
            .is_error,
            Some(false)
        );
        assert_eq!(
            s.docx_extract_markdown(Parameters(TextExtractParams {
                path: "x".into(),
                bytes: b64(b"x"),
                word_from: None,
                word_to: None
            }))
            .await
            .unwrap()
            .is_error,
            Some(false)
        );

        let s2 = svc(DocumentKind::Xlsx);
        assert_eq!(
            s2.xlsx_info(Parameters(SpreadsheetInfoParams {
                path: "x".into(),
                bytes: b64(b"x")
            }))
            .await
            .unwrap()
            .is_error,
            Some(false)
        );
        assert_eq!(
            s2.xlsx_extract_markdown(Parameters(SpreadsheetExtractParams {
                path: "x".into(),
                bytes: b64(b"x"),
                sheet: None,
                row_from: None,
                row_to: None
            }))
            .await
            .unwrap()
            .is_error,
            Some(false)
        );

        let s3 = svc(DocumentKind::Pptx);
        assert_eq!(
            s3.pptx_info(Parameters(PresentationInfoParams {
                path: "x".into(),
                bytes: b64(b"x")
            }))
            .await
            .unwrap()
            .is_error,
            Some(false)
        );
        assert_eq!(
            s3.pptx_extract_markdown(Parameters(PresentationExtractParams {
                path: "x".into(),
                bytes: b64(b"x"),
                slide_from: None,
                slide_to: None
            }))
            .await
            .unwrap()
            .is_error,
            Some(false)
        );
    }

    #[tokio::test]
    async fn cross_kind_rejections() {
        // docx_info on a PDF must fail.
        let s = svc(DocumentKind::Pdf);
        assert_eq!(
            s.docx_info(Parameters(TextInfoParams {
                path: "x".into(),
                bytes: b64(b"x")
            }))
            .await
            .unwrap()
            .is_error,
            Some(true)
        );
        assert_eq!(
            s.xlsx_info(Parameters(SpreadsheetInfoParams {
                path: "x".into(),
                bytes: b64(b"x")
            }))
            .await
            .unwrap()
            .is_error,
            Some(true)
        );
        assert_eq!(
            s.pptx_info(Parameters(PresentationInfoParams {
                path: "x".into(),
                bytes: b64(b"x")
            }))
            .await
            .unwrap()
            .is_error,
            Some(true)
        );
        assert_eq!(
            s.pdf_info(Parameters(PdfInfoParams {
                path: "x".into(),
                bytes: b64(b"x")
            }))
            .await
            .unwrap()
            .is_error,
            Some(false)
        );
    }
}

#[cfg(test)]
mod annotation_tests {
    //! Verify M04: every tool carries per-tool Annotations (readOnlyHint,
    //! destructiveHint, idempotentHint, openWorldHint) plus a title and a
    //! per-tool description carrying usage guidance. The `#[tool(...)]` macro
    //! generates a `<name>_tool_attr()` associated function returning the
    //! `rmcp::model::Tool` metadata; we assert on those.

    use rmcp::model::ToolAnnotations;

    use super::DocumentsServer;

    /// Expected annotations for every read-only extraction tool.
    fn expect_read_only(a: &Option<ToolAnnotations>, tool: &str) {
        let ann = a
            .as_ref()
            .unwrap_or_else(|| panic!("{tool}: missing annotations"));
        assert_eq!(ann.read_only_hint, Some(true), "{tool}: readOnlyHint");
        assert_eq!(ann.destructive_hint, Some(false), "{tool}: destructiveHint");
        assert_eq!(ann.idempotent_hint, Some(true), "{tool}: idempotentHint");
        assert_eq!(ann.open_world_hint, Some(false), "{tool}: openWorldHint");
        assert!(
            ann.title.as_deref().is_some_and(|t| !t.is_empty()),
            "{tool}: annotations.title must be a non-empty human-readable title"
        );
    }

    /// Expected: a non-empty `description` (the per-tool guidance channel).
    fn expect_description(d: &Option<std::borrow::Cow<'static, str>>, tool: &str) {
        assert!(
            d.as_deref().is_some_and(|s| !s.is_empty()),
            "{tool}: description must carry per-tool guidance"
        );
    }

    #[test]
    fn all_tools_carry_read_only_annotations() {
        let tools = [
            DocumentsServer::classify_document_tool_attr(),
            DocumentsServer::pdf_info_tool_attr(),
            DocumentsServer::pdf_extract_markdown_tool_attr(),
            DocumentsServer::doc_info_tool_attr(),
            DocumentsServer::doc_extract_markdown_tool_attr(),
            DocumentsServer::docx_info_tool_attr(),
            DocumentsServer::docx_extract_markdown_tool_attr(),
            DocumentsServer::rtf_info_tool_attr(),
            DocumentsServer::rtf_extract_markdown_tool_attr(),
            DocumentsServer::xls_info_tool_attr(),
            DocumentsServer::xls_extract_markdown_tool_attr(),
            DocumentsServer::xlsx_info_tool_attr(),
            DocumentsServer::xlsx_extract_markdown_tool_attr(),
            DocumentsServer::ppt_info_tool_attr(),
            DocumentsServer::ppt_extract_markdown_tool_attr(),
            DocumentsServer::pptx_info_tool_attr(),
            DocumentsServer::pptx_extract_markdown_tool_attr(),
            DocumentsServer::csv_info_tool_attr(),
            DocumentsServer::csv_extract_markdown_tool_attr(),
        ];

        assert_eq!(tools.len(), 19, "all 19 tools must be covered");
        for tool in &tools {
            let name = tool.name.to_string();
            expect_read_only(&tool.annotations, &name);
            expect_description(&tool.description, &name);
        }
    }
}
