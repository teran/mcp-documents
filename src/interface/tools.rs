//! MCP tool parameter types and result/error formatting helpers.

use rmcp::model::{CallToolResult, ContentBlock};

use crate::domain::error::ExtractionError;
use crate::domain::result::Source;

/// Parameters for `classify_pdf`.
#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ClassifyParams {
    /// Path to the PDF file to classify.
    pub path: String,
    /// Optional base64-encoded PDF content (used when the client passes the
    /// file bytes to the model instead of a path). Takes precedence over
    /// `path` when present.
    #[serde(default)]
    #[schemars(description = "Optional base64-encoded PDF bytes")]
    pub bytes: Option<String>,
}

/// Parameters for `extract_pdf` / `extract_pdf_text`.
#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ExtractParams {
    /// Path to the PDF file to extract.
    pub path: String,
    /// Optional base64-encoded PDF content (used when the client passes the
    /// file bytes to the model instead of a path). Takes precedence over
    /// `path` when present.
    #[serde(default)]
    #[schemars(description = "Optional base64-encoded PDF bytes")]
    pub bytes: Option<String>,
}

/// Build a [`Source`] from the tool params: prefer base64 `bytes` if present
/// (decoding them), otherwise fall back to the file `path`.
pub fn source_from_params(path: &str, bytes: Option<&str>) -> Result<Source, ExtractionError> {
    match bytes {
        Some(b64) => {
            use base64::Engine as _;
            let data = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| ExtractionError::Malformed(format!("invalid base64 bytes: {e}")))?;
            Ok(Source::Bytes(data))
        }
        None => Ok(Source::Path(std::path::PathBuf::from(path))),
    }
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

    #[test]
    fn source_from_params_uses_path_when_no_bytes() {
        let s = source_from_params("/tmp/a.pdf", None).unwrap();
        match s {
            Source::Path(p) => assert_eq!(p, std::path::PathBuf::from("/tmp/a.pdf")),
            _ => panic!("expected Path"),
        }
    }

    #[test]
    fn source_from_params_decodes_base64_bytes() {
        // base64 of b"%PDF-1.4 test"
        let s = source_from_params("/tmp/x.pdf", Some("JVBERi0xLjQgdGVzdA==")).unwrap();
        match s {
            Source::Bytes(data) => assert_eq!(data, b"%PDF-1.4 test"),
            _ => panic!("expected Bytes"),
        }
    }

    #[test]
    fn source_from_params_invalid_base64_is_malformed() {
        let e = source_from_params("/tmp/x.pdf", Some("!!!not-base64!!!")).unwrap_err();
        assert!(matches!(e, ExtractionError::Malformed(_)));
    }
}
