//! Interface / adapter layer: the MCP server (built on `rmcp`) and the CLI.
//!
//! This is the **only** layer that may import `rmcp` types
//! (`CallToolResult`, `ErrorData`, `#[tool]`, ...).

pub mod cli;
pub mod server;
pub mod tools;

pub use cli::{CliCommand, run_cli};
pub use server::{DocumentsServer, serve_stdio};
