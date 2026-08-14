//! MCP tool parameter types and result/error formatting helpers.

use rmcp::model::{CallToolResult, ContentBlock};

use crate::domain::error::ExtractionError;

/// Parameters for `classify_pdf`.
#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ClassifyParams {
    /// Path to the PDF file to classify.
    pub path: String,
}

/// Parameters for `extract_pdf` / `extract_pdf_text`.
#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ExtractParams {
    /// Path to the PDF file to extract.
    pub path: String,
}

/// Format a successful tool result from plain text.
pub fn text_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text.into())])
}

/// Format a JSON-serializable payload as a successful tool result.
pub fn json_result(value: impl serde::Serialize) -> CallToolResult {
    let text = rmcp::serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// Turn a domain [`ExtractionError`] into a **tool-level** error result (the
/// caller sees a readable message). Bad *document data* belongs here, as
/// opposed to a protocol error for malformed invocations.
pub fn tool_error(e: &ExtractionError) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!("{e}"))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::document::DocumentKind;

    #[test]
    fn text_result_is_success() {
        let r = text_result("hello");
        assert_eq!(r.is_error, Some(false));
        assert_eq!(r.content.len(), 1);
    }

    #[test]
    fn json_result_serializes() {
        let r = json_result(rmcp::serde_json::json!({"a": 1}));
        assert_eq!(r.is_error, Some(false));
        assert!(r.content[0].as_text().is_some());
    }

    #[test]
    fn tool_error_is_error_result() {
        let r = tool_error(&ExtractionError::Unsupported(DocumentKind::Pdf));
        assert_eq!(r.is_error, Some(true));
    }
}
