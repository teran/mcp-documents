# mcp-documents

An **MCP (Model Context Protocol) server** that extracts clean text and Markdown from documents.

Built in **Rust** with a **Domain-Driven / Clean Architecture**, it currently supports **PDF** text extraction — including **OCR for scanned PDFs** — and is designed to extend to Microsoft Office and other formats in a future round.

---

## Features

- 📄 **PDF extraction** — text-based PDFs converted to clean GitHub-Flavored Markdown (tables, multi-column layout, position-aware content).
- 🔍 **PDF classification** — detect whether a PDF is `TextBased`, `Scanned`, `ImageBased`, or `Mixed`, and which pages need OCR.
- 🖼️ **OCR for scanned PDFs** — scanned/image-only pages routed through a pluggable OCR provider (reference: **`tesseract-rs`** with **embedded tessdata** — English + Russian — no external process or system Tesseract).
- 🔌 **Pluggable architecture** — domain & application layers are decoupled from the MCP framework and the document library, making the OCR provider and even the extraction engine swappable.
- 📦 **Single self-contained binary** — zero runtime system dependencies; OCR (vendored Tesseract) and PDF rendering are embedded/static so nothing needs to be installed at runtime.
- 🧪 **Tested** — 85%+ line-coverage target enforced via `cargo llvm-cov` on a bare CI runner.

> **Extraction engine:** `anydoc` was validated empirically against alternatives (7/7 real files converted, including legacy `.doc`/`.xls`/`.ppt`), and is the confirmed single primary engine.

---

## Quick Start

### Prerequisites

- Rust toolchain (stable) with `cargo`.
- **Local build (default features):** pure Rust — **no** C++ compiler / CMake / OCR data needed.
- **Full OCR build** (the shipped self-contained binary): needs a C++ compiler + CMake + internet at build time (to compile the vendored Tesseract+Leptonica and download `rus.traineddata`).
- **No runtime system packages required.** The server ships as a **single self-contained binary**: OCR runs on `tesseract-rs` with **embedded tessdata** (English + Russian) and PDF rendering is pure Rust. There is **no** runtime Tesseract, `libtesseract`/`libleptonica`, or `tessdata` to install — nothing is shelled out. (The binary dynamically links only glibc/libstdc++, so the Docker runtime image matches the builder's Debian ABI.)

### Build

The heavy OCR stack (`tesseract-rs`) is gated behind the **`ocr-native` cargo feature (default OFF)** so local dev stays pure-Rust. The full self-contained OCR binary is built via **Docker multi-stage**, which pre-provisions `rus.traineddata` and sets `TESSERACT_EMBED_LANGUAGES=eng,rus` (the default omits Russian).

```bash
# local dev — default features, pure Rust, no C++ toolchain
cargo build
cargo test

# full OCR binary — recommended (Docker multi-stage → self-contained)
docker build -t mcp-documents .

# full OCR binary — build locally with the wrapper
make build        # TESSERACT_EMBED_LANGUAGES=eng,rus cargo build --release --features ocr-native
```

> The default `cargo build` produces a working binary, but with the `ocr-native` feature **off** OCR returns `OcrUnavailable` for scanned PDFs. Build with `--features ocr-native` (or via `make build`/Docker) for full OCR support.

### Run as an MCP stdio server

```bash
cargo run --release
```

The server speaks MCP over **stdio** — launch it from your MCP client rather than a normal terminal. (Running the binary with no subcommand, or with `serve`, starts the MCP server.)

### Standalone CLI

The same binary doubles as a CLI for quick local testing (no MCP client needed):

```bash
mcp-documents classify <file.pdf>       # print PDF classification + pages needing OCR
mcp-documents extract <file.pdf>        # print extracted Markdown
mcp-documents extract-text <file.pdf>   # print extracted plain text
mcp-documents serve                     # run the MCP stdio server (also the default)
```

---

## Configuring MCP clients (stdio)

Because the transport is **stdio**, add the binary as a stdio MCP server in your client's config.

### Claude Code (`claude mcp add` or `.mcp.json`)

```json
{
  "mcpServers": {
    "documents": {
      "command": "/absolute/path/to/mcp-documents/target/release/mcp-documents"
    }
  }
}
```

### opencode (`opencode.json`)

```json
{
  "mcp": {
    "documents": {
      "type": "local",
      "command": ["/absolute/path/to/mcp-documents/target/release/mcp-documents"],
      "enabled": true
    }
  }
}
```

> Always use an **absolute path** to the built binary. Rebuild with `cargo build --release` after changes, then restart the client.

---

## Available tools

| Tool | Description |
|------|-------------|
| `classify_pdf` | Returns the PDF type (`TextBased` / `Scanned` / `ImageBased` / `Mixed`) plus the list of pages needing OCR. |
| `extract_pdf` | Extracts the document as **Markdown** (local extraction for text pages, OCR for scanned pages, results merged). |
| `extract_pdf_text` | Plain-text variant of `extract_pdf`. |

All tools accept a file **path** (and/or raw **bytes** depending on the final API). See [`SPEC.md`](./SPEC.md) §8.2 for details.

---

## Development workflow

```bash
# format
cargo fmt

# lint
cargo clippy --all-targets -- -D warnings

# test
cargo test

# coverage (≥ 85% line target)
cargo install cargo-llvm-cov        # once
cargo llvm-cov --html
```

See [`AGENTS.md`](./AGENTS.md) for the layer/dependency rules and conventions AI agents must follow in this repo.

---

## Project layout

```
src/
├── main.rs              # stdio entrypoint (Interface layer)
├── lib.rs
├── domain/              # entities, value objects, ports (traits), domain errors
├── application/         # use cases: classify + extract (route & merge)
├── infrastructure/      # adapters: anydoc extractor, tesseract-rs OCR (embedded tessdata), PDF renderer, file reader, error map
└── interface/           # MCP server built on rmcp (#[tool] methods)
tests/
└── fixtures/            # sample documents for integration tests
```

Refer to [`SPEC.md`](./SPEC.md) §5.2 for the full module map.

---

## Roadmap

- **Round 1 (current):** PDF classification, extraction, OCR routing — shipped as a single self-contained binary.
- **Round 2 (next):** Microsoft Office (Word / Excel / PowerPoint) and optional OpenDocument / RTF / EPUB / CSV — largely enabled by the existing `anydoc` integration.

---

## License

Licensed under the **Apache License 2.0**. See [`LICENSE`](./LICENSE).
