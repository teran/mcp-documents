# AGENTS.md — Instructions for AI coding agents

Working on this Rust MCP server. Read this before making changes.

## Project at a glance

- **What:** an MCP server extracting text/Markdown from documents — PDF (with OCR) **plus** MS Office / ODF / RTF / EPUB / CSV — shipping as a **single self-contained binary** (embedded OCR data; dynamically links glibc/libstdc++ only).
- **Stack:** Rust, `tokio`, `rmcp` (MCP server framework), `anydoc` (document → Markdown, uses `pdf-inspector` internally for PDF), `tesseract-rs` (vendored OCR with embedded tessdata) + a pure-Rust PDF rasterizer.
- **Architecture:** DDD / Clean Architecture (ports & adapters).
- **Zero runtime system deps:** no runtime Tesseract / libtesseract / libleptonica / tessdata, and nothing is shelled out. (Build time needs a C++ compiler + CMake to compile the vendored Tesseract+Leptonica.)
- **Coverage gate:** ≥ 85% **line** coverage via `cargo llvm-cov` on a bare CI runner.
- See [`SPEC.md`](./SPEC.md) for the full technical specification and [`README.md`](./README.md) for user-facing docs.

## Layer map (module → layer) — do not blur these boundaries

| Module | Layer | Responsibilities | May depend on |
|--------|-------|------------------|---------------|
| `src/domain/` | **Domain** | Entities/value objects (`DocumentKind` — Pdf/Doc/Docx/Xls/Xlsx/Ppt/Pptx/Odt/Ods/Odp/Rtf/Epub/Csv/Unknown, `PdfType`, `ExtractionResult`), domain errors (`ExtractionError`), **ports** (`DocumentExtractor`, `OcrProvider`, `PdfPageRenderer`) | Nothing external |
| `src/application/` | **Application** | Use cases (`ClassifyDocumentUseCase`, `ExtractDocumentUseCase`): classify → route (PDF text vs OCR vs non-PDF straight-through `anydoc`) → merge | Domain ports only |
| `src/infrastructure/` | **Infrastructure** | Adapters implementing ports (`AnyDocExtractor`, `PdfPageRenderer`, plus `TesseractOcrProvider` when `ocr-native` is on / `NoopOcrProvider` when off), file reader, error mapping | Implements domain ports; may import `anydoc`, `pdf-render`, and (only behind `ocr-native`) `tesseract-rs` |
| `src/interface/` | **Interface** | MCP server (`rmcp` `#[tool]` methods), dual-mode entrypoint (`main.rs`: CLI dispatch vs stdio), CLI (`cli.rs`) | Application use cases |

## Hard dependency rules (do not violate)

1. **Domain and Application layers must import NO framework/library code** — no `rmcp`, no `anydoc`, no `pdf-inspector`. Domain/application code must compile without those dependencies on the dependency graph.
2. Application code interacts with documents **only through ports** (`DocumentExtractor`, `OcrProvider`) — never by calling `anydoc`/`pdf-inspector` directly.
3. **Never** import a concrete infrastructure adapter into the application layer. Inject them (dependency inversion).
4. `rmcp` types (`CallToolResult`, `McpError`, `#[tool]`) appear **only** in `src/interface/`. Keep domain errors out of the protocol layer except at the translation boundary.
5. Library errors (`anydoc::ConvertError`, etc.) must be translated into domain `ExtractionError` in `infrastructure/error_map.rs` — never leak library error types into application/interface.

## Error handling conventions

- **Tool-level error** (`Ok(CallToolResult::error(...))`) for *expected business failures*: unsupported/malformed/encrypted input, OCR unavailable, resource limit. The caller should see a readable message.
- **Protocol error** (`Err(McpError::invalid_params(...))`) only for *malformed invocations* (bad params/wrong types).
- Rule of thumb: **bad document data → tool-level; bad request shape → protocol error.**
- Scanned PDF with no OCR provider available → return `ExtractionError::OcrUnavailable`; **never** silently return partial/empty content.
- **Panic isolation:** wrap document-conversion/parse calls in `std::panic::catch_unwind` (per-file / per-request) in the infrastructure layer. A malformed document (e.g. `pdf-extract`/`cff-parser` panicking on embedded fonts) must become a tool-level domain error, **never** abort the MCP request/process.

## Key implementation facts

- `anydoc`: `anydoc::to_markdown(path)?`, `anydoc::to_markdown_bytes(&bytes, None)?`, `anydoc::to_document(&bytes, None)?`; detection via `Format::from_bytes/from_extension/from_path`. Returns `Err` only when no meaningful Markdown can be produced. **Confirmed as the single primary engine** (empirically: 7/7 real files, incl. legacy `.doc`/`.xls`/`.ppt`); `markitdown-rs` is rejected as primary (quality + robustness + legacy gaps — see SPEC §4.1.1).
- `rmcp`: `#[tool_router(server_handler)]` + `#[tool(description = "...")]`; `impl` methods take `Parameters<Params>` where `Params: serde::Deserialize + schemars::JsonSchema`; stdio via `Server.serve(stdio()).await?; service.waiting().await?;`.
- **Dual-mode binary:** `main.rs` dispatches on argv — recognized subcommands (`classify`, `extract`, `extract-text`, `serve`) run the CLI (`interface::cli`), otherwise the MCP stdio server runs. Both share the same use cases and adapters.
- **Multi-format:** `DocumentKind` is Pdf/Doc/Docx/Xls/Xlsx/Ppt/Pptx/Odt/Ods/Odp/Rtf/Epub/Csv/Unknown. `classify_document`/`extract_document` handle **any** `anydoc`-supported format (via `path` or base64 `bytes`); PDF-specific `classify_pdf`/`extract_pdf`/`extract_pdf_text` remain. **PDF is the only OCR path** — non-PDF formats go straight through `anydoc` (no OCR). CLI `classify`/`extract`/`extract-text` work for any supported format.
- **`.xls`/`.xlsb` labeling caveat:** legacy binary `.doc`/`.ppt`/`.xls` extract correctly (anydoc/calamine), but `.xls` is labeled `Xlsx` — Excel formats aren't distinguishable from bytes alone.
- **No OCR in `anydoc`/`pdf-inspector`** — they only extract text from *text-based* PDFs, and neither renders PDF pages to images. OCR goes through the `OcrProvider` port (reference: `TesseractOcrProvider`).
- **OCR is `tesseract-rs` (embed-tessdata), gated behind the `ocr-native` feature (default OFF):** `tesseract-rs` is an **optional dependency**; the default build is pure-Rust (a `NoopOcrProvider` returns `OcrUnavailable`), and the real `TesseractOcrProvider` is `#[cfg(feature="ocr-native")]`. Tesseract+Leptonica are **vendored and compiled from source at build time**, and `eng`+`rus` tessdata is **embedded into the binary** (`TESSERACT_EMBED_LANGUAGES=eng,rus`; the default `eng,tur` does **not** include Russian). **Gotcha:** `api.init_embedded("eng+rus")` does **NOT** work (the embedded map is keyed per single language → `None` → `InitError`); instead **materialize the embedded bytes into a temp dir** (via `get_embedded_tessdata`/`embedded_languages`) and call `api.init(temp_dir, "eng+rus")?`. `api.try_clone()` enables parallel OCR. The `OcrProvider` consumes **`PageBitmap` values** (`ocr_page(bitmap: &PageBitmap)`) — the renderer+OCR port exchange a `PageBitmap { width, height, bytes_per_pixel, data }` value object so Tesseract's `set_image` gets true geometry (guessing a square page breaks non-square A4 scans). Produced by the `PdfPageRenderer` step (`pdf_render`) that runs *before* OCR (see SPEC §4.4/§9). All of this is infrastructure-layer only and statically links into one self-contained binary.
- **Renderer is `pdf_render` (finalized):** pure Rust (pdf-interpret + skrifa + ttf-parser + image/png + vello_cpu), statically linkable, no native/-sys deps. Render via `Pdf::new` + `InterpreterSettings` + `RenderSettings { x_scale, y_scale, bg_color }` at scale 300/72; extract grayscale from `pixmap.data_as_u8_slice()` (premultiplied RGBA8 → luma). `Pdf::new` returns `LoadPdfError` (needs `{:?}`, no `Display`). **`rasterrocket` is REJECTED** (native FreeType via pkg-config + openjpeg-sys; dynamically links `libfreetype.so.6`; broken crates.io release).
- **Build-time vs runtime deps:** the **default build is pure-Rust** (no C++/CMake needed); the **full OCR build** (`--features ocr-native`) needs a C++ compiler + CMake + internet (to compile/download vendored Tesseract/Leptonica). **Runtime needs none** of those. The full build runs in a **Docker multi-stage build** (builder pre-provisions `rus.traineddata` + sets `TESSERACT_EMBED_LANGUAGES=eng,rus` + `--features ocr-native`; runtime stage ships only the binary). Download `rus.traineddata` from a **mirror (e.g. jsdelivr)** or a host copy — GitHub raw can be flaky.
- **Runtime base ABI:** the binary is **not fully static** — it dynamically links glibc + libstdc++ (Tesseract/Leptonica are static, but glibc/libstdc++ are dynamic). The Docker runtime stage uses **`debian:trixie-slim`**, which **must match the builder's Debian release** (a trixie-built binary needs `GLIBC_2.38`/`GLIBCXX_3.4.31`, missing from `bookworm`/glibc 2.36). A fully static musl build is out of scope for R1.
- **Gotcha — `rus` is NOT auto-downloaded:** `tesseract-rs`'s `build.rs` hardcodes the download list to `["eng","tur"]` and downloads **`tessdata_best`** (accurate; eng+rus ≈ 30.7 MB, ~22 MB over `fast`). `TESSERACT_EMBED_LANGUAGES` only selects which **already-downloaded** files to embed — it does NOT trigger downloads. **Pre-provision `rus.traineddata`** into the tesseract-rs tessdata cache dir (macOS: `~/Library/Application Support/tesseract-rs/tessdata/`, Linux: `~/.tesseract-rs/tessdata/`) **before** `cargo build`, or embedding `rus` silently fails ("Language rus not found"). The Dockerfile provisions `tessdata_best` (accuracy); switch to `tessdata_fast` if binary size matters (≈ 22 MB smaller).
- **OCR concurrency:** at startup, **materialize the embedded tessdata into a temp dir** and init once with `api.init(temp_dir, "eng+rus")?`; keep a **pool of `try_clone()` instances** sized to CPU cores (Tesseract init ~0.5–1 s, so avoid per-page init), and run each OCR through **`tokio::task::spawn_blocking`** (OCR is blocking/not async-safe). Enables page-parallel OCR + concurrent requests. R1 fallback: per-request fresh init.
- **No runtime system deps and no shell-out** — never invoke an external binary or link system libraries at runtime. Build the shipped artifact in release mode.
- Prefer bytes mode / content-based detection over trusting caller-supplied extensions. Enforce size limits (map to `ResourceLimit`).

## Testing expectations (≥ 85% line coverage)

- **Unit-test domain + application** with fake/stub ports (in-memory `FakeExtractor`, `FakeOcrProvider`, `FakePageRenderer`) — no real OCR/render, no filesystem. This is where OCR routing/merge/error logic is covered. Keep heavy logic in pure, fake-testable functions; keep the thin ML/glue small.
- **Integration-test infrastructure adapters** against committed fixtures in `tests/fixtures/` (text-based PDF, scanned PDF, mixed PDF, malformed, encrypted, oversized).
- Because OCR uses `tesseract-rs` with **embedded tessdata** (no runtime system deps), the real OCR integration tests **run normally on any machine** — they are **not** skipped for lack of a system Tesseract. (The Docker `ocr-native` build must pre-provision `rus.traineddata` and embed `eng`+`rus`.)
- Use `#[tokio::test]` for async use cases; `#[cfg(test)]` modules for unit tests.
- Interface/tools tested by invoking handlers with constructed params, asserting `CallToolResult` shape and error classification.

## Commands

```bash
# default build (pure Rust, no C++ toolchain) / local tests
cargo build
cargo test

# full OCR build (needs C++ + CMake + rus.traineddata + internet)
make build                    # sets TESSERACT_EMBED_LANGUAGES=eng,rus + --features ocr-native
cargo build --release --features ocr-native

# fmt / lint
cargo fmt --check             # or: cargo fmt
cargo clippy --all-targets -- -D warnings

# coverage (CI gate runs on the ocr-native build; must pass ≥ 85 lines)
cargo llvm-cov --html
cargo llvm-cov --fail-under-lines 85
```

Run `cargo fmt`, `cargo clippy` with `-D warnings`, and the 85% coverage gate before considering work complete.

## Do-not-violate checklist

- [ ] No `rmcp`/`anydoc`/`pdf-inspector` imports in `src/domain/` or `src/application/`.
- [ ] Application uses ports, not concrete adapters.
- [ ] Library errors mapped to `ExtractionError` in the infrastructure layer.
- [ ] OCR runs via the **`tesseract-rs`** engine with `embed-tessdata` (`eng`+`rus`), gated behind the **`ocr-native` feature (default OFF)** — **never** shell out to a `tesseract` CLI, and never require a system `libtesseract`/`tessdata` at runtime; `OcrProvider` consumes rendered page bitmaps only.
- [ ] The shipped artifact is a **single self-contained binary** with zero runtime OCR/system deps (no native renderer/OCR libs at runtime); it dynamically links only glibc/libstdc++ — the Docker runtime base must match the builder ABI.
- [ ] The build (Docker multi-stage) **pre-provisions `rus.traineddata`** and sets `TESSERACT_EMBED_LANGUAGES=eng,rus` + `--features ocr-native`; never rely on the crate's auto-download for `rus`.
- [ ] Default `cargo build`/`cargo test` stays **pure-Rust** (no C++/CMake) — heavy OCR only behind `ocr-native`.
- [ ] Multi-format extraction (MS Office/ODF/RTF/EPUB/CSV) goes through `anydoc` via the generic `classify_document`/`extract_document`; **PDF is the only OCR path**; `.xls` labels as `Xlsx` (known caveat).
- [ ] Coverage stays ≥ 85% lines.
- [ ] Follow the file/module layout in SPEC §5.2; keep domain/application library-agnostic.

Refer to [`SPEC.md`](./SPEC.md) for architecture, ports, tool API, and roadmap details.
