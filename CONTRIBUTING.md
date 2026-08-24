# Contributing

Thanks for your interest in `mcp-documents`! This is a Rust **MCP server** that
extracts text/Markdown from documents (PDF with OCR, plus MS Office / ODF / RTF
/ EPUB / CSV), shipping as a single self-contained binary.

Please read [`AGENTS.md`](./AGENTS.md) for the detailed layer/dependency rules
and [`SPEC.md`](./SPEC.md) for the full technical specification before making
changes.

## Building

The heavy OCR stack (`tesseract-rs`) is gated behind the **`ocr-native`** cargo
feature (**default OFF**), so day-to-day development stays pure-Rust.

```bash
# default build — pure Rust, no C++/CMake needed
cargo build
cargo test

# full OCR build (self-contained binary) — needs C++ + CMake + internet
# and pre-provisioned rus.traineddata
make build                     # TESSERACT_EMBED_LANGUAGES=eng,rus cargo build --release --features ocr-native
docker build -t mcp-documents .  # or build via the multi-stage Dockerfile
```

> **`ocr-native` prerequisite:** the crate only auto-downloads `eng`/`tur`
> tessdata. To embed Russian you must pre-provision `rus.traineddata` into the
> tesseract-rs cache dir (`make prepare-tessdata` does this; see SPEC §4.3).

## Running checks (CI gate)

```bash
cargo fmt --check                          # formatting
cargo clippy --all-targets -- -D warnings  # lint (warnings denied)
cargo test                                 # default build tests
cargo build --release                      # ensure release compiles
cargo llvm-cov --fail-under-lines 85       # coverage gate (default build)
cargo llvm-cov --features ocr-native --fail-under-lines 85  # coverage gate (ocr-native)
```

The **≥85% line-coverage gate runs on BOTH the default and `ocr-native`
builds** — please run both before opening a PR.

## Architecture & layer rules (summary)

The codebase follows DDD / Clean Architecture with strict boundaries. Do not
blur them:

| Layer | Module | Responsibility |
|-------|--------|----------------|
| Domain | `src/domain/` | Entities/value objects, **ports** (traits), domain errors |
| Application | `src/application/` | Use cases: classify → route per format → merge |
| Infrastructure | `src/infrastructure/` | Concrete adapters implementing the ports |
| Interface | `src/interface/` | MCP server (`rmcp`) + CLI |

Hard rules:

- **Domain and Application must import NO business library code** (`rmcp`,
  `anydoc`, `pdf-inspector`, `pdf-render`, `csv`, `calamine`, `zip`,
  `quick-xml`, `tesseract-rs`). Application interacts with documents only
  through ports and **may** use `tokio::task`.
- **Never import a concrete infrastructure adapter into the application layer** —
  inject them (dependency inversion).
- **`rmcp` types appear only in `src/interface/`.** Library errors must be
  translated into domain `ExtractionError` in `src/infrastructure/error_map.rs`.
- **Tool-level errors for bad document data; protocol errors for bad request
  shape.** Wrap document parsing/conversion in `catch_unwind` (panic isolation)
  so a malformed document becomes a tool-level error, never a crash.
- **Tool surface is per-format:** `classify_document` + `<format>_info` /
  `<format>_extract_markdown` for pdf/doc/docx/rtf/xls/xlsx/ppt/pptx/csv,
  Markdown-only output. Do **not** reintroduce the old
  `classify_pdf`/`extract_pdf`/`extract_pdf_text`/`extract_document` tools.
- OCR runs via the `tesseract-rs` engine with embedded `eng`+`rus` tessdata
  (behind `ocr-native`); **never shell out** to a `tesseract` CLI and never
  require a system `libtesseract`/`tessdata` at runtime. Non-PDF formats go
  through their dedicated adapters (no OCR). `.xls` vs `.xlsx` is distinguished
  by magic bytes.

See [`AGENTS.md`](./AGENTS.md) for the complete "do-not-violate" checklist.

## Adding a format / adapter

1. **Domain** — add the `DocumentKind` variant (and any needed value objects)
   and define a **port** (trait) for the new format's reader/extractor.
2. **Application** — add a use case (or extend an existing one) that routes via
   the port: classify → extract → merge. Keep it library-agnostic.
3. **Infrastructure** — implement the port as a concrete adapter (e.g. a new
   `XxxReader`); translate library errors into `ExtractionError`.
4. **Interface** — wire up the `<format>_info` / `<format>_extract_markdown`
   `#[tool]` methods.
5. **Tests** — unit-test the use case with a fake port; integration-test the
   adapter against fixtures in `tests/fixtures/` (or generated in-memory files).
   Keep coverage ≥ 85%.

## Code style

- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` (warnings are
  errors) before submitting.
- Keep heavy logic in pure, fake-testable functions; keep thin adapter glue
  small.
- Follow the existing module layout (SPEC §5.2).

## Reporting issues & submitting PRs

- **Bugs / questions / features** → open a GitHub issue. Please include the
  reproduction, the document kind, and the exact error/`RUST_BACKTRACE`.
- **Security vulnerabilities** → **do not** open a public issue. Report
  privately via **GitHub Security Advisories** (see [`SECURITY.md`](./SECURITY.md)).
- **PRs** → keep changes focused, run the full check suite (fmt, clippy, tests,
  both coverage gates), and reference any related issue. CI runs
  fmt/clippy/test/coverage/ocr-native/audit/deny automatically.

The maintainers will review promptly. Thanks for contributing!
