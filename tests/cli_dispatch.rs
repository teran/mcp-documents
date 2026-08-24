//! End-to-end tests of the binary's argv dispatch.
//!
//! These spawn the compiled `mcp-documents` binary and drive its CLI
//! subcommands (`classify` / `extract`) against the committed fixtures. This
//! exercises the real `main` entrypoint (the `CliDispatch::Cli` branch) and the
//! full adapter stack without needing OCR or any system dependencies.
//!
//! The `serve` path (MCP stdio) is intentionally not spawned here: it blocks on
//! stdin. It is covered by the dispatch unit tests in `src/interface/cli.rs`.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcp-documents")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run(args: &[&str]) -> std::process::Output {
    // Point ALLOW_ROOT_DIR at the repo root so the committed fixtures are
    // inside the sandbox (the default root is ~/Documents, which the fixtures
    // would fall outside of).
    Command::new(bin())
        .env("ALLOW_ROOT_DIR", env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn classify_subcommand_prints_kind_for_fixtures() {
    for name in ["simple_text.docx", "simple_text.pdf", "simple_text.xlsx"] {
        let out = run(&["classify", fixture(name).to_str().unwrap()]);
        assert!(
            out.status.success(),
            "classify {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("kind:"),
            "classify {name} did not print a kind: {stdout}"
        );
    }
}

#[test]
fn classify_pdf_reports_pdf_type() {
    let out = run(&["classify", fixture("simple_text.pdf").to_str().unwrap()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pdf_type:"), "missing pdf_type: {stdout}");
}

#[test]
fn extract_subcommand_prints_markdown_for_fixtures() {
    let cases = [
        ("simple_text.docx", "Hello docx from mcp-documents"),
        ("simple_text.pdf", "Hello, world from mcp-documents"),
        ("simple_text.xlsx", "Hello xlsx from mcp-documents"),
    ];
    for (name, needle) in cases {
        let out = run(&["extract", fixture(name).to_str().unwrap()]);
        assert!(
            out.status.success(),
            "extract {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(needle),
            "extract {name} missing {needle:?}: {stdout}"
        );
    }
}

#[test]
fn extract_missing_file_returns_error() {
    let out = run(&["extract", "/nonexistent/definitely-missing.docx"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error"),
        "expected an error message, got: {stderr}"
    );
}

#[test]
fn classify_missing_file_returns_error() {
    let out = run(&["classify", "/nonexistent/definitely-missing.pdf"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error"),
        "expected an error message, got: {stderr}"
    );
}
