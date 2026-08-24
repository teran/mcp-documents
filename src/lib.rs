//! `mcp-documents` — an MCP server that extracts text/Markdown from documents.
//!
//! Multi-format: PDF (with per-page classification and OCR routing for scanned
//! pages) plus MS Office / ODF / RTF / EPUB / CSV. Each format exposes an
//! `_info` and `_extract_markdown` tool pair; the output is Markdown only. The
//! crate follows DDD / Clean Architecture; see [`domain`], [`application`],
//! [`infrastructure`] and [`interface`].

pub mod application;
pub mod config;
pub mod domain;
pub mod infrastructure;
pub mod interface;
