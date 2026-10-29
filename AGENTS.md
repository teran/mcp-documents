# AGENTS.md — Instructions for AI coding agents

Working on this Rust MCP server. Read this before making changes.

## Project at a glance

- **What:** an MCP server extracting text/Markdown from documents — PDF (with OCR) **plus** MS Office / RTF / CSV — shipping as a **single self-contained binary** (embedded OCR data; dynamically links glibc/libstdc++ only).
- **Stack:** Rust, `tokio`, `rmcp` (MCP server framework), `pdf-inspector` (per-page PDF text + classification), `pdf-render` (pure-Rust PDF→image rasterizer), `anydoc` (word-processor word counting + legacy `.ppt` + ODF/EPUB fallback), `csv` (streaming), `calamine` (xls/xlsx), `zip` + `quick-xml` (PPTX slides), `tesseract-rs` (vendored OCR with embedded eng+rus tessdata).
- **Architecture:** DDD / Clean Architecture (ports & adapters).
- **Zero runtime system deps:** no runtime Tesseract / libtesseract / libleptonica / tessdata, and nothing is shelled out. (Build time needs a C++ compiler + CMake to compile the vendored Tesseract+Leptonica.)
- **Coverage gate:** ≥ 95% **line** coverage on the default build (≥ 85% on the `ocr-native` build) via `cargo llvm-cov` on a bare CI runner.
- See [`SPEC.md`](./SPEC.md) for the full technical specification and [`README.md`](./README.md) for user-facing docs.

## Layer map (module → layer) — do not blur these boundaries

| Module | Layer | Responsibilities | May depend on |
|--------|-------|------------------|---------------|
| `src/domain/` | **Domain** | Entities/value objects (`DocumentKind` — Pdf/Doc/Docx/Xls/Xlsx/Ppt/Pptx/Odt/Ods/Odp/Rtf/Epub/Csv/Unknown, `PdfType`, `Classification`, `ExtractionResult`, `PageBitmap`), metadata (`meta.rs`), ranges (`ranges.rs`), domain errors (`ExtractionError`), **ports** (`DocumentExtractor`, `PdfTextSource`, `TextDocumentReader`, `SpreadsheetReader`, `PresentationReader`, `CsvReader`, `OcrProvider`, `PdfPageRenderer`) | Nothing external |
| `src/application/` | **Application** | Use cases (`ClassifyDocumentUseCase`, `PdfUseCase`, `TextDocUseCase`, `SpreadsheetUseCase`, `PresentationUseCase`, `CsvUseCase`): classify → route per format → merge | Domain ports only; **may use `tokio::task`** (async) |
| `src/infrastructure/` | **Infrastructure** | Adapters implementing ports (`AnyDocExtractor`, `PdfTextSourceAdapter`, `PdfRenderer`, `AnyDocTextReader`, `CalamineSpreadsheetReader`, `AnyDocPresentationReader`, `CsvStreamReader`, plus `TesseractOcrProvider` when `ocr-native` is on / `NoopOcrProvider` when off), file reader + `ALLOW_ROOT_DIR` validation, error mapping | Implements domain ports; may import `anydoc`, `pdf-inspector`, `pdf-render`, `csv`, `calamine`, `zip`, `quick-xml`, and (only behind `ocr-native`) `tesseract-rs` |
| `src/interface/` | **Interface** | MCP server (`rmcp` `#[tool]` methods), dual-mode entrypoint (`main.rs`: CLI dispatch vs stdio), CLI (`cli.rs`) | Application use cases |

## Hard dependency rules (do not violate)

1. **Domain and Application layers must import NO business library code** — no `rmcp`, no `anydoc`, no `pdf-inspector`, no `pdf-render`, no `csv`, no `calamine`, no `zip`/`quick-xml`, no `tesseract-rs`. Domain/application code must compile without those dependencies on the dependency graph.
2. Application code interacts with documents **only through ports** (`DocumentExtractor`, `PdfTextSource`, `TextDocumentReader`, `SpreadsheetReader`, `PresentationReader`, `CsvReader`, `OcrProvider`, `PdfPageRenderer`) — never by calling a library directly.
3. **The application layer is async and `tokio::task` is EXPLICITLY ALLOWED there** (e.g. `spawn_blocking`, `JoinSet`, `Semaphore` for the PDF OCR concurrency). The ban on business libraries does **not** extend to `tokio` task primitives.
4. **Never** import a concrete infrastructure adapter into the application layer. Inject them (dependency inversion).
5. `rmcp` types (`CallToolResult`, `ErrorData`, `#[tool]`) appear **only** in `src/interface/`. Keep domain errors out of the protocol layer except at the translation boundary.
6. Library errors (`anydoc::ConvertError`, `pdf_inspector::PdfError`, `pdf_render::LoadPdfError`, `calamine::Error`, `zip::result::ZipError`, ...) must be translated into domain `ExtractionError` in `infrastructure/error_map.rs` — never leak library error types into application/interface.

## Error handling conventions

- **Tool-level error** (`Ok(CallToolResult::error(...))`) for *expected business failures*: unsupported/malformed/encrypted input, OCR unavailable, resource limit, wrong document kind for a per-format tool. The caller should see a readable message.
- **Protocol error** (`Err(McpError::invalid_params(...))`) only for *malformed invocations* (bad params/wrong types).
- Rule of thumb: **bad document data → tool-level; bad request shape → protocol error.**
- Scanned PDF with no OCR provider available → return `ExtractionError::OcrUnavailable`; **never** silently return partial/empty content.
- **Panic isolation:** wrap document-conversion/parse calls in `std::panic::catch_unwind` (per-file / per-request) in the infrastructure layer. A malformed document (e.g. `pdf-inspector`/`pdf-render` panicking on embedded fonts) must become a tool-level domain error, **never** abort the MCP request/process.

## Key implementation facts

- **Tool surface is per-format:** one generic `classify_document` plus, for each of `pdf`/`doc`/`docx`/`rtf`/`xls`/`xlsx`/`ppt`/`pptx`/`csv`, an `_info` tool (JSON metadata) and an `_extract_markdown` tool (Markdown). Output is **Markdown only** — there are no plain-text tools. Ranges: pdf `page_from`/`page_to`; doc/docx/rtf `word_from`/`word_to`; xls/xlsx `sheet`+`row_from`/`row_to`; ppt/pptx `slide_from`/`slide_to` (`.ppt` whole-doc, range ignored); csv `row_from`/`row_to`. **The old tools `classify_pdf`, `extract_pdf`, `extract_pdf_text`, `extract_document` are GONE.** All tools accept `path` + optional base64 `bytes`. **All tools are read-only** (they only read/parse documents) and are declared with explicit per-tool Annotations via `#[tool(annotations(...))]` (`readOnlyHint: true`, `destructiveHint: false`, `idempotentHint: true`, `openWorldHint: false`) plus a `title` and per-tool usage guidance in the description. The MCP spec has no per-tool `instructions` field — per-tool guidance lives in `description`; `instructions` is server-level only (see SPEC §8.2, M04).
- **PDF is the only OCR path.** PDFs use `pdf-inspector` **per page** (text + `TextBased`/`Scanned`/`ImageBased`/`Mixed` classification + `pages_needing_ocr`); scanned pages are rendered with `pdf_render` and OCR'd. A **Mixed** PDF interleaves text and OCR pages in page order (per-page interleaving is **resolved** — no longer a limitation).
- **Non-PDF formats go through dedicated adapters, no OCR:** CSV = streaming `csv` crate (`csv_info` + `row_from`/`row_to`, memory < file size); xls/xlsx = `calamine` (sheets + `sheet`/`row_range`, `.xls` vs `.xlsx` distinguished by **magic bytes**); pptx = `zip`+`quick-xml` slide parts (`slide_range`), legacy binary `.ppt` = `anydoc` whole-document (no slide range, `slide_count: 1`); doc/docx/rtf = word/char counting via `anydoc` (`word_range` + `MAX_RETURN_CHARS` safety net). ODF/EPUB are classified and (CLI) whole-doc fallback via `anydoc`.
- **Dual-mode binary:** `main.rs` dispatches on argv — recognized subcommands (`classify`, `extract`, `serve`) run the CLI (`interface::cli`), otherwise the MCP stdio server runs. Both share the same use cases and adapters. (There is **no** `extract-text` subcommand.)
- **No OCR in `anydoc`/`pdf-inspector`/`pdf-render`** — they extract text / render pages but do not OCR. OCR goes through the `OcrProvider` port (reference: `TesseractOcrProvider`).
- **OCR is `tesseract-rs` (embed-tessdata), gated behind the `ocr-native` feature (default OFF):** `tesseract-rs` is an **optional dependency**; the default build is pure-Rust (a `NoopOcrProvider` returns `OcrUnavailable`), and the real `TesseractOcrProvider` is `#[cfg(feature="ocr-native")]`. Tesseract+Leptonica are **vendored and compiled from source at build time**, and `eng`+`rus` tessdata is **embedded into the binary** (`TESSERACT_EMBED_LANGUAGES=eng,rus`; the default `eng,tur` does **not** include Russian). **Gotcha:** `api.init_embedded("eng+rus")` does **NOT** work (the embedded map is keyed per single language → `None` → `InitError`); instead **materialize the embedded bytes into a stable per-user cache dir** (via `get_embedded_tessdata`/`embedded_languages`) and call `api.init(cache_dir, "eng+rus")?`. `api.try_clone()` enables parallel OCR. The `OcrProvider` consumes **`PageBitmap` values** (`ocr_page(bitmap: &PageBitmap)`) — the renderer+OCR port exchange a `PageBitmap { width, height, bytes_per_pixel, data }` value object so Tesseract's `set_image` gets true geometry (guessing a square page breaks non-square A4 scans). Produced by the `PdfPageRenderer` step (`pdf_render`) that runs *before* OCR (see SPEC §4.3/§9). All of this is infrastructure-layer only and statically links into one self-contained binary.
- **OCR concurrency is REAL and bounded (implemented, not just claimed):** `PdfUseCase::ocr_pages_concurrently` runs scanned pages in parallel bounded by a **semaphore sized to `OCR_CONCURRENCY`** (default CPU cores) via a `tokio::task::JoinSet` of `spawn_blocking` workers, each rendering then OCRing a page with a **`try_clone()`** of the single initialized API. The number of OCR'd pages per request is **capped by `OCR_MAX_PAGES`** (default 300) → `ResourceLimit`.
- **Renderer is `pdf_render` (finalized):** pure Rust (pdf-interpret + skrifa + ttf-parser + image/png + vello_cpu), statically linkable, no native/-sys deps. Render via `Pdf::new` + `InterpreterSettings` + `RenderSettings { x_scale, y_scale, bg_color }` at scale 300/72; extract grayscale from `pixmap.data_as_u8_slice()` (premultiplied RGBA8 → luma). `Pdf::new` returns `LoadPdfError` (needs `{:?}`, no `Display`). A render cap (`MAX_RENDER_PIXELS` = 100M px, `MAX_RENDER_SIDE` = 15k px) guards against giant pixmaps. **`rasterrocket` is REJECTED** (native FreeType via pkg-config + openjpeg-sys; dynamically links `libfreetype.so.6`; broken crates.io release).
- **Build-time vs runtime deps:** the **default build is pure-Rust** (no C++/CMake needed); the **full OCR build** (`--features ocr-native`) needs a C++ compiler + CMake + internet (to compile/download vendored Tesseract/Leptonica). **Runtime needs none** of those. The full build runs in a **Docker multi-stage build** (builder pre-provisions `rus.traineddata` + sets `TESSERACT_EMBED_LANGUAGES=eng,rus` + `--features ocr-native`; runtime stage ships only the binary). Download `rus.traineddata` from a **mirror (e.g. jsdelivr)** or a host copy — GitHub raw can be flaky.
- **Runtime base ABI:** the binary is **not fully static** — it dynamically links glibc + libstdc++ (Tesseract/Leptonica are static, but glibc/libstdc++ are dynamic). The Docker runtime stage uses **`gcr.io/distroless/cc-debian13:nonroot`** and runs as **`nobody` (UID 65534)** with a writable **`/cache`** (`XDG_CACHE_HOME=/cache`). The base **must match the builder's Debian release** (a trixie-built binary needs `GLIBC_2.38`/`GLIBCXX_3.4.31`, missing from `cc-debian12`/glibc 2.36); the `cc` variant is required because it bundles libstdc++. A fully static musl build is out of scope for R1.
- **Gotcha — `rus` is NOT auto-downloaded:** `tesseract-rs`'s `build.rs` hardcodes the download list to `["eng","tur"]` and downloads **`tessdata_best`** (accurate; eng+rus ≈ 30.7 MB, ~22 MB over `fast`). `TESSERACT_EMBED_LANGUAGES` only selects which **already-downloaded** files to embed — it does NOT trigger downloads. **Pre-provision `rus.traineddata`** into the tesseract-rs tessdata cache dir (macOS: `~/Library/Application Support/tesseract-rs/tessdata/`, Linux: `~/.tesseract-rs/tessdata/`) **before** `cargo build`, or embedding `rus` silently fails ("Language rus not found"). The Dockerfile provisions `tessdata_best` (accuracy); switch to `tessdata_fast` if binary size matters (≈ 22 MB smaller).
- **OCR startup cache:** at startup, **materialize the embedded tessdata into a stable per-user cache dir** (`$XDG_CACHE_HOME/mcp-documents/tessdata` → `~/.cache/mcp-documents/tessdata` → temp fallback) and init once with `api.init(cache_dir, "eng+rus")?`; on later startups it **size-checks** existing `.traineddata` files (fast `stat`, idempotent + self-healing, no lock) and only rewrites if missing/size differs. Each OCR runs through **`tokio::task::spawn_blocking`** (OCR is blocking/not async-safe).
- **Config env vars (no `MCP_` prefix; read at call time):** `MAX_INPUT_BYTES` (default 250 MiB, input-size cap → `ResourceLimit`), `OCR_MAX_PAGES` (default 300, per-request page cap → `ResourceLimit`), `OCR_CONCURRENCY` (default = CPU cores, bounds OCR parallelism), `MAX_RETURN_CHARS` (default 50M, returned-output cap for all extractors → `ResourceLimit`), `MAX_DECOMPRESSED_BYTES` (default 256 MiB, cap on a single decompressed archive member, e.g. a PPTX slide part → `ResourceLimit`), `ALLOW_ROOT_DIR` (default `~/Documents` on Unix with `~` expansion, `/` fallback when no home; all paths canonicalized + must be within it; the container sets `ALLOW_ROOT_DIR=/data`).
- **No runtime system deps and no shell-out** — never invoke an external binary or link system libraries at runtime. Build the shipped artifact in release mode.
- Prefer bytes mode / content-based detection over trusting caller-supplied extensions. Enforce size limits (map to `ResourceLimit`).

## Testing expectations (default ≥ 95% line coverage; `ocr-native` ≥ 85%)

- **Unit-test domain + application** with fake/stub ports (in-memory `FakeExtractor`, `FakeOcrProvider`, `FakePageRenderer`, `FakePdfTextSource`, and fake text/spreadsheet/presentation/CSV readers) — no real OCR/render, no filesystem. This is where OCR routing/cap/concurrency, merge, and error logic are covered. Keep heavy logic in pure, fake-testable functions; keep the thin adapter glue small.
- **Integration-test infrastructure adapters** against the committed fixtures in `tests/fixtures/` (`simple_text.docx`, `simple_text.pdf`, `simple_text.xlsx`) plus generated in-memory files (temp CSVs, PPTX zips built in `tests/format_adapters.rs`).
- **Fixture gap:** the repo currently has **no** dedicated scanned/mixed/malformed/encrypted PDF fixtures committed. Those paths are covered by unit tests and the `ocr-native` synthetic test. Do **not** assume fixtures that don't exist; if you need them, add them (or note it).
- Because OCR uses `tesseract-rs` with **embedded tessdata** (no runtime system deps), the real OCR integration tests **run normally on any machine** — they are **not** skipped for lack of a system Tesseract. (The Docker `ocr-native` build must pre-provision `rus.traineddata` and embed `eng`+`rus`.)
- **`ocr-native` test:** `tests/ocr_native.rs` (gated `#![cfg(feature="ocr-native")]`) runs the real `TesseractOcrProvider` + `PdfRenderer` on a rendered **synthetic scanned page** and asserts non-empty Cyrillic-capable OCR. The gate is **≥ 95% on the default build** (measured ~97%) and **≥ 85% on the `ocr-native` build** (measured ~91%).
- Use `#[tokio::test]` for async use cases (e.g. `PdfUseCase`); `#[cfg(test)]` modules for unit tests.
- Interface/tools tested by invoking handlers with constructed params, asserting `CallToolResult` shape and error classification (including cross-kind rejections — e.g. `docx_info` on a PDF returns a tool-level `unsupported`).

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

# coverage (default-build gate ≥ 95 lines; ocr-native gate ≥ 85)
cargo llvm-cov --html
cargo llvm-cov --fail-under-lines 95
```

Run `cargo fmt`, `cargo clippy` with `-D warnings`, and the 95% coverage gate (default build) / 85% gate (`ocr-native`) before considering work complete.

## Do-not-violate checklist

- [ ] No `rmcp`/`anydoc`/`pdf-inspector`/`pdf-render`/`csv`/`calamine`/`zip`/`quick-xml`/`tesseract-rs` imports in `src/domain/`. No business-library imports in `src/application/` (application **may** use `tokio::task`).
- [ ] Application uses ports, not concrete adapters.
- [ ] Library errors mapped to `ExtractionError` in the infrastructure layer.
- [ ] OCR runs via the **`tesseract-rs`** engine with `embed-tessdata` (`eng`+`rus`), gated behind the **`ocr-native` feature (default OFF)** — **never** shell out to a `tesseract` CLI, and never require a system `libtesseract`/`tessdata` at runtime; `OcrProvider` consumes rendered page bitmaps only.
- [ ] The shipped artifact is a **single self-contained binary** with zero runtime OCR/system deps (no native renderer/OCR libs at runtime); it dynamically links only glibc/libstdc++ — the Docker runtime base (`cc-debian13`) must match the builder ABI.
- [ ] The build (Docker multi-stage) **pre-provisions `rus.traineddata`** and sets `TESSERACT_EMBED_LANGUAGES=eng,rus` + `--features ocr-native`; never rely on the crate's auto-download for `rus`.
- [ ] Default `cargo build`/`cargo test` stays **pure-Rust** (no C++/CMake) — heavy OCR only behind `ocr-native`.
- [ ] Tool surface stays **per-format**: `classify_document` + `<format>_info` / `<format>_extract_markdown` for pdf/doc/docx/rtf/xls/xlsx/ppt/pptx/csv; Markdown-only output; **do not reintroduce** `classify_pdf`/`extract_pdf`/`extract_pdf_text`/`extract_document`.
- [ ] `.xls` vs `.xlsx` distinguished by **magic bytes** (not labeled all `Xlsx`); **PDF is the only OCR path**; non-PDF formats go through their dedicated adapters (no OCR).
- [ ] OCR concurrency stays **bounded** by `OCR_CONCURRENCY` and `OCR_MAX_PAGES`; per-request OCR page count capped → `ResourceLimit`.
- [ ] Coverage stays ≥ 95% lines on the default build (≥ 85% on `ocr-native`).
- [ ] Follow the file/module layout in SPEC §5.2; keep domain/application library-agnostic.

Refer to [`SPEC.md`](./SPEC.md) for architecture, ports, tool API, config, and roadmap details.
