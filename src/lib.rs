//! `mcp-documents` — an MCP server that extracts text/Markdown from documents.
//!
//! Round 1 scope: PDF classification + extraction with OCR routing for scanned
//! pages. The crate follows DDD / Clean Architecture; see [`domain`],
//! [`application`], [`infrastructure`] and [`interface`].

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod interface;
