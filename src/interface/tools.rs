//! MCP tool parameter types and result/error formatting helpers.
//!
//! Also centralizes source construction + validation (input-size limit and
//! `ALLOWROOT`), so every tool enforces the same constraints.

use std::path::PathBuf;

use rmcp::model::{CallToolResult, ContentBlock};

use crate::config::max_input_bytes;
use crate::domain::error::ExtractionError;
use crate::domain::result::Source;
use crate::infrastructure::validate_source;

/// Common `path` + optional base64 `bytes` fields shared by every tool.
macro_rules! source_params {
    ($(#[$meta:meta])* $name:ident { $($extra:tt)* }) => {
        $(#[$meta])*
        #[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
        pub struct $name {
            /// Path to the document file.
            pub path: String,
            /// Optional base64-encoded document content (used when the client
            /// passes the file bytes to the model instead of a path). Takes
            /// precedence over `path` when present.
            #[serde(default)]
            #[schemars(description = "Optional base64-encoded document bytes")]
            pub bytes: Option<String>,
            $($extra)*
        }
    };
}

source_params! {
    /// Parameters for `classify_document`.
    ClassifyParams {}
}

source_params! {
    /// Parameters for `pdf_info`.
    PdfInfoParams {}
}

source_params! {
    /// Parameters for `pdf_extract_markdown` (0-based page range, end exclusive).
    PdfExtractParams {
        /// First page (0-based) to include. Defaults to the first page.
        #[serde(default)]
        pub page_from: Option<usize>,
        /// Page after the last one (0-based, exclusive) to include. Defaults to
        /// the page after the last page.
        #[serde(default)]
        pub page_to: Option<usize>,
    }
}

source_params! {
    /// Parameters for `doc_info` / `docx_info` / `rtf_info`.
    TextInfoParams {}
}

source_params! {
    /// Parameters for `doc_extract_markdown` / `docx_extract_markdown` /
    /// `rtf_extract_markdown` (0-based word range, end exclusive).
    TextExtractParams {
        /// First word (0-based) to include. Defaults to the first word.
        #[serde(default)]
        pub word_from: Option<usize>,
        /// Word after the last one (0-based, exclusive) to include. Defaults to
        /// the word after the last word.
        #[serde(default)]
        pub word_to: Option<usize>,
    }
}

source_params! {
    /// Parameters for `xls_info` / `xlsx_info`.
    SpreadsheetInfoParams {}
}

source_params! {
    /// Parameters for `xls_extract_markdown` / `xlsx_extract_markdown`.
    SpreadsheetExtractParams {
        /// Sheet name to extract. Defaults to the first sheet.
        #[serde(default)]
        pub sheet: Option<String>,
        /// First row (0-based) to include. Defaults to the first row.
        #[serde(default)]
        pub row_from: Option<usize>,
        /// Row after the last one (0-based, exclusive) to include. Defaults to
        /// the row after the last row.
        #[serde(default)]
        pub row_to: Option<usize>,
    }
}

source_params! {
    /// Parameters for `ppt_info` / `pptx_info`.
    PresentationInfoParams {}
}

source_params! {
    /// Parameters for `ppt_extract_markdown` / `pptx_extract_markdown`
    /// (0-based slide range, end exclusive).
    PresentationExtractParams {
        /// First slide (0-based) to include. Defaults to the first slide.
        #[serde(default)]
        pub slide_from: Option<usize>,
        /// Slide after the last one (0-based, exclusive) to include. Defaults to
        /// the slide after the last slide.
        #[serde(default)]
        pub slide_to: Option<usize>,
    }
}

source_params! {
    /// Parameters for `csv_info`.
    CsvInfoParams {}
}

source_params! {
    /// Parameters for `csv_extract_markdown` (0-based row range over data rows,
    /// end exclusive; the header row is always shown).
    CsvExtractParams {
        /// First data row (0-based) to include. Defaults to the first data row.
        #[serde(default)]
        pub row_from: Option<usize>,
        /// Data row after the last one (0-based, exclusive) to include. Defaults
        /// to the row after the last data row.
        #[serde(default)]
        pub row_to: Option<usize>,
    }
}

/// Build a [`Source`] from the tool params: prefer base64 `bytes` if present
/// (decoding them), otherwise fall back to the file `path`.
///
/// Applies `MAX_INPUT_BYTES` **before** base64 decode (via a decoded-size
/// estimate) and `ALLOWROOT` validation + canonicalization for paths.
pub fn source_from_params(path: &str, bytes: Option<&str>) -> Result<Source, ExtractionError> {
    let source = match bytes {
        Some(b64) => {
            let trimmed = b64.trim();
            // Pre-decode guard: 4 base64 chars -> up to 3 bytes. Prevent a
            // giant decode allocation before we even decode.
            if (trimmed.len() / 4) * 3 > max_input_bytes() {
                return Err(ExtractionError::ResourceLimit(format!(
                    "base64 input decodes to more than MAX_INPUT_BYTES={}",
                    max_input_bytes()
                )));
            }
            use base64::Engine as _;
            let data = base64::engine::general_purpose::STANDARD
                .decode(trimmed)
                .map_err(|e| ExtractionError::Malformed(format!("invalid base64 bytes: {e}")))?;
            Source::Bytes(data)
        }
        None => Source::Path(PathBuf::from(path)),
    };
    // Validate the input size and (for paths) ALLOWROOT; canonicalize paths.
    validate_source(&source)
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
    use crate::config::ENV_LOCK;
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
    fn source_from_params_decodes_base64_bytes() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let s = source_from_params("/tmp/x.pdf", Some("JVBERi0xLjQgdGVzdA==")).unwrap();
        match s {
            Source::Bytes(data) => assert_eq!(data, b"%PDF-1.4 test"),
            _ => panic!("expected Bytes"),
        }
    }

    #[test]
    fn source_from_params_invalid_base64_is_malformed() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let e = source_from_params("/tmp/x.pdf", Some("!!!not-base64!!!")).unwrap_err();
        assert!(matches!(e, ExtractionError::Malformed(_)));
    }

    #[test]
    fn source_from_params_enforces_size_before_decode() {
        use base64::Engine as _;
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("MAX_INPUT_BYTES", "10") };
        // base64 of >10 bytes decoded.
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 100]);
        let err = source_from_params("/tmp/x.pdf", Some(&big)).unwrap_err();
        unsafe { std::env::remove_var("MAX_INPUT_BYTES") };
        assert!(matches!(err, ExtractionError::ResourceLimit(_)));
    }

    #[test]
    fn source_from_params_validates_missing_path() {
        let err = source_from_params("/definitely/missing/x.pdf", None).unwrap_err();
        assert!(matches!(err, ExtractionError::Io(_)));
    }
}
