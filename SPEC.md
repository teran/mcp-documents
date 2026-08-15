# Technical Specification — `mcp-documents`

> An MCP (Model Context Protocol) server that extracts clean text and Markdown from documents.

- **Language:** Rust
- **Runtime:** `tokio` (async)
- **Architecture:** DDD / Clean Architecture (ports & adapters)
- **Status:** Multi-format — PDF text extraction (incl. OCR for scanned PDFs) **plus** MS Office / ODF / RTF / EPUB / CSV.

---

## 1. Goals

The server extracts clean text/Markdown from a broad set of document formats. The system must be able to:

1. **Classify** a document (PDF or any supported format) — for PDFs, identify `TextBased` / `Scanned` / `ImageBased` / `Mixed` and which pages require OCR.
2. **Extract** text/Markdown from *text-based* PDFs (tables, multi-column layouts, position-aware content) **and** from MS Office / ODF / RTF / EPUB / CSV formats (via `anydoc`).
3. **OCR** scanned / image-only **PDF** pages via a pluggable OCR provider and **merge** the results back into a coherent document (non-PDF formats go straight through `anydoc`, no OCR).
4. Expose these capabilities to MCP clients (e.g. Claude Code, opencode) over **stdio** as typed tools.
5. Maintain a **line-coverage target of ≥ 85%**.

PDF+OCR was the founding capability; multi-format (MS Office etc.) is now enabled as a natural extension — a wider `DocumentKind`, an `extract_document`/`classify_document` tool pair, and no new document library (all via `anydoc`).

## 2. Non-Goals

- ❌ No editing, conversion *to* PDF, or document generation.
- ❌ No rendering / thumbnailing of pages.
- ❌ No network service / HTTP transport (stdio only for now).
- ❌ **No external OCR process or system OCR at runtime** — OCR runs via `tesseract-rs` with its `embed-tessdata` feature (Tesseract+Leptonica vendored and compiled at build time; language data **embedded in the binary**). Nothing is shelled out; no system `tesseract` / `libtesseract` / `libleptonica` / `tessdata` is required at runtime.
- ❌ No runtime system dependencies — the shipped artifact is a **single self-contained binary** (embedded OCR data; dynamically links glibc/libstdc++ only) with **zero** installed packages required.
- ❌ No persistence / caching of extracted documents.

## 3. Scope

### 3.1 PDF + OCR (foundation, implemented)

- PDF classification (`classify_pdf`).
- PDF → Markdown extraction, text-based path (`extract_pdf`).
- PDF → plain text extraction, text-based path (`extract_pdf_text`).
- OCR routing for scanned/image pages via `OcrProvider` port + reference `TesseractOcrProvider` adapter (`tesseract-rs` with **embedded tessdata**, English + Russian).
- MCP stdio server (`rmcp`).

### 3.2 Multi-format (R2, implemented)

- **MS Office + more, all via `anydoc`:** Word (`doc`, `docx`), Excel (`xls`, `xlsx`), PowerPoint (`ppt`, `pptx`), plus OpenDocument (`odt`/`ods`/`odp`), RTF, EPUB, CSV.
- Generic tools **`classify_document`** and **`extract_document`** accept any supported format (via `path` or base64 `bytes`); PDF-specific tools are retained.
- **Routing:** PDF keeps the classify → render → OCR → merge path; **non-PDF formats go straight through `anydoc` (no OCR)**.
- **Known limitation:** legacy binary `.doc`/`.ppt`/`.xls` extract correctly (anydoc/calamine), but **`.xls` is labeled `Xlsx`** — Excel formats are not distinguishable from bytes alone.
- Because we standardize on `anydoc`, no new document library was required; the change was a wider `DocumentKind` enum, the generic tool pair, and extended fixture/test matrices.

## 4. Library Choice: `anydoc` (recommended) vs `pdf-inspector`

### 4.1 Recommendation: use `anydoc`

| Criterion | `anydoc` | `pdf-inspector` |
|-----------|----------|-----------------|
| Scope | Multi-format (Word, PPT, Excel, ODF, RTF, EPUB, CSV **and** PDF) | PDF only |
| PDF engine | Wraps `pdf-inspector` internally | Native |
| License | MIT | — |
| ML / external services | None (pure Rust) | None |
| Output | Clean GitHub-Flavored Markdown | Markdown + position data |

**Why `anydoc`:** it satisfies our **entire** goal set with one consistent API and one consistent Markdown output. Multi-format (MS Office + ODF/RTF/EPUB/CSV) is covered with a **single integration**; PDFs are handled through `pdf-inspector` under the hood, so we do not lose PDF fidelity.

#### 4.1.1 Decision evidence (empirical spike)

A release-mode spike compared `anydoc` (v0.1.8) against `markitdown-rs` (crate `markitdown`, v0.1.11) on 7 real files (RU bank-terms PDF, 7.4 MB English O'Reilly PDF, 2 RU DOCX, 2 XLSX, 27 MB RU PPTX). Measured result:

| Dimension | `anydoc` | `markitdown-rs` |
|-----------|----------|-----------------|
| Files converted | **7/7** | **5/7** |
| Embedded-font PDF | OK | **PANIC** (`cff-parser`/`pdf-extract` decoding Type1/CFF fonts — aborts the request thread if unisolated) |
| 27 MB PPTX | Real slide content | **Mis-detected as generic `.zip`**, dumped raw OOXML XML |
| PDF / DOCX quality | Excellent — tables with columns, headings, lists, links, reading order; Cyrillic fine | PDF tables flattened into scrambled prose; DOCX words fragmented mid-word across lines; single cells split into multiple columns |
| XLSX | Correct tables | Correct tables (tie) |
| PPTX | Real slide content | Fails (zip misdetection) |
| Legacy `.doc` / `.ppt` / `.xls` | Native support (binary `.xls` included) | **No `.doc`/`.ppt` converters**; `.xls` uses a zip/XML reader that cannot parse binary `.xls` |
| Coverage | PDF + docx/pptx/xlsx + ODF/RTF/EPUB/CSV | Subset |

**Conclusion:** `anydoc` is **confirmed as the single primary engine**. `markitdown-rs` is **rejected as primary** (quality + robustness + legacy-format gaps); it may remain only as a theoretical optional adapter, and is **not recommended**.

### 4.2 When to fall back to `pdf-inspector` directly

`pdf-inspector` remains the **underlying PDF engine** and the **fallback** if we later need:

- Lower-level control over PDF parsing.
- Direct access to position-aware data / per-page metadata beyond what `anydoc` exposes.
- Finer-grained OCR routing (it provides classification confidence scores and `pages_needing_ocr`).

Because both libraries are isolated behind the `DocumentExtractor` port, swapping between them is a **pure infrastructure-layer change** with no impact on domain/application logic.

### 4.3 OCR engine choice: `tesseract-rs` (primary, feature-gated)

The MCP server must ship as a **single self-contained binary with ZERO runtime system dependencies** — no installed Tesseract, no `libtesseract`/`libleptonica`, no `tessdata` files on disk, and no external binary invoked. **`tesseract-rs`** (crate `tesseract-rs`, cafercangundogdu, v0.4.0, MIT) with its **`embed-tessdata`** feature satisfies this: Tesseract + Leptonica are **vendored and compiled from source at build time**, and the language data is **embedded into the binary**, so the runtime needs no system OCR and no `tessdata` directory.

**Feature gate — `ocr-native` (default OFF):** to keep local `cargo build`/`cargo test` **pure-Rust** (no C++ compiler / CMake / `rus.traineddata` needed), the heavy OCR dependency `tesseract-rs` is an **optional dependency** gated behind the cargo feature **`ocr-native`** (default OFF).

- **Default build (pure Rust):** `rmcp`, `anydoc`, `pdf_render`, `tokio`, `serde`, `schemars` — compiles and tests locally with **no C++ toolchain**.
- **With `--features ocr-native`:** enables `tesseract-rs` (`embed-tessdata`). The `TesseractOcrProvider` adapter is `#[cfg(feature="ocr-native")]`; when the feature is off, a `NoopOcrProvider` returns `ExtractionError::OcrUnavailable`, so the OCR routing compiles and behaves correctly in both configurations.
- **Full OCR binary** is built in Docker/CI with the feature on (the Dockerfile sets `TESSERACT_EMBED_LANGUAGES=eng,rus`, pre-provisions `rus.traineddata`, and builds with `--features ocr-native`). See §4.3.4 / §12.

#### 4.3.1 Primary: `tesseract-rs` (embed-tessdata)

- **Crate:** `tesseract-rs = { version = "0.4.0", features = ["embed-tessdata"] }`.
- **Engine:** compiles Tesseract + Leptonica from source at **build time** (vendored). Runtime needs **no** system `tesseract` / `libtesseract` / `libleptonica` / `tessdata` — stays a single self-contained binary.
- **Languages:** embedded at build time via the env var `TESSERACT_EMBED_LANGUAGES=eng,rus cargo build`. The **default is English + Turkish (`eng, tur`) — Russian is NOT included by default**, so the build **MUST set `TESSERACT_EMBED_LANGUAGES=eng,rus`** to support the Russian documents this server targets.
- **Runtime init — gotcha:** `init_embedded("eng+rus")` does **NOT** work: the embedded tessdata map is keyed **per single language**, so a `"eng+rus"` lookup returns `None` → `InitError`. **Workaround (still zero external runtime deps):** at startup, materialize the embedded bytes (via `get_embedded_tessdata` / `embedded_languages`) into a **temp directory**, then call `init(temp_dir, "eng+rus")`. The bytes come from the embedded binary, not disk/network, so the single-binary / zero-runtime-dep property is preserved. Availability can be queried via `api.embedded_languages()`.
- **Concurrency:** `api.try_clone()` clones the API for **parallel OCR** — relevant to the `spawn_blocking` / concurrency design (each worker OCRs its own cloned instance).
- **Build note:** always build in **release mode**; the vendored C++ build is long on first compile (the crate caches compiled libraries for faster subsequent builds). The build runs inside a **Docker multi-stage build** for a portable, reproducible environment (see §4.3.4).
- **tessdata variant & size:** the crate's `build.rs` downloads **`tessdata_best`** (hardcoded `.../tessdata_best/...` base URL) — the accurate LSTM variant. Measured sizes: `best` eng ≈ 15.4 MB / rus ≈ 15.3 MB (eng+rus ≈ 30.7 MB), vs `fast` eng ≈ 4.1 MB / rus ≈ 3.9 MB (eng+rus ≈ 8 MB). We ship **`tessdata_best`** for OCR accuracy (the Dockerfile provisions it); `tessdata_fast` is the smaller alternative (≈ 22 MB less) if binary size ever becomes critical.
- **Gotcha — `rus` is NOT auto-downloaded:** `build.rs` hardcodes the download list to `["eng", "tur"]`. `TESSERACT_EMBED_LANGUAGES` only selects which **already-downloaded** files to embed; it does **not** trigger downloads. Embedding `rus` therefore **silently fails** ("Language rus not found") unless `rus.traineddata` is physically present in the tesseract-rs tessdata cache dir **before** the build runs.

#### 4.3.2 Alternative (not primary): `ocrs`

`ocrs` (pure-Rust ML OCR with embedded models) was previously the primary; it remains a one-line **alternative if `tesseract-rs` accuracy proves insufficient** on target scans. It does not require a C++/CMake build step, but was superseded by `tesseract-rs` as the chosen primary. If used, model data is embedded into the binary (e.g. `ocrs-models`).

#### 4.3.3 What we do NOT use

- **`leptess`** and the **`tesseract`** crate are **dropped** — they require system-installed `libtesseract` + `tessdata` at runtime, which violates the zero-runtime-dependency constraint.
- **No external `tesseract` binary** is ever invoked (no shelling out).

The `OcrProvider` port keeps this decision swappable: swapping `TesseractOcrProvider` for an `ocrs`-based provider is a pure infrastructure-layer change with no impact on domain/application logic.

#### 4.3.4 Build tooling (Docker multi-stage)

The env var and the `rus.traineddata` provisioning are easy to get wrong, so the build is performed in a **Docker multi-stage build** — this lets us **fully control the build environment** (e.g. place `rus.traineddata` wherever needed) and is **portable/reproducible** anywhere, at the cost of being less convenient than a local build.

- **Builder stage:** a C++ compiler + CMake image; **pre-provision `rus.traineddata`** into the tesseract-rs tessdata cache dir (macOS: `~/Library/Application Support/tesseract-rs/tessdata/`, Linux: `~/.tesseract-rs/tessdata/`) — download it from a **pinned tessdata source** into that cache dir **before** `cargo build`; set `TESSERACT_EMBED_LANGUAGES=eng,rus`; run `cargo build --release --features ocr-native` (via the wrapper, `make build` / `just build`).
- **`rus.traineddata` download source:** GitHub raw can be flaky for downloading `rus.traineddata` at build time — the Docker builder should use a **mirror (e.g. jsdelivr)** or a **host copy** of the file instead.
- **Runtime stage:** uses **`debian:trixie-slim`** (or any base that **matches the builder's glibc/libstdc++ ABI**); **copy only the single self-contained binary**. Zero OCR/system packages are added, nothing shelled out, embedded `eng`+`rus` tessdata.
- **ABI note:** the binary is **dynamically linked against glibc + libstdc++** (Tesseract/Leptonica are statically linked, but glibc/libstdc++ are dynamic). A trixie-built binary needs `GLIBC_2.38` / `GLIBCXX_3.4.31`, which `debian:bookworm-slim` (glibc 2.36) lacks — hence the runtime base **must match the builder's Debian release**. A fully static binary would require a musl build, which is **out of scope for Round 1**.
- The crate also needs **internet** in the builder (to download Tesseract 5.5.2 + Leptonica 1.87.0 sources + tessdata) and a C++ compiler + CMake; **none** of these are needed at runtime.

### 4.4 PDF page → image renderer choice: `pdf_render` (finalized)

The OCR engine consumes **bitmap images**, not PDFs, so the pipeline needs a PDF page → image renderer that is **pure Rust** (to keep the single self-contained binary). This was settled by a **Docker-based spike** (base `rust:1.97.1-trixie`) comparing `pdf_render` vs `rasterrocket`, both feeding `tesseract-rs` (eng+rus) on 5 real Russian scanned PDFs at 300 DPI.

#### 4.4.1 Result: `pdf_render` confirmed

- Both succeeded on all PDFs and Cyrillic came through in both, but **`pdf_render` was cleaner**: it read a small-font receipt correctly that `rasterrocket` garbled (`Чек 1`: pdf_render read "Яндекс … Итого 97.00 Р", rasterrocket produced garbage), and it kept Latin/Cyrillic word shapes intact where `rasterrocket` mangled them.
- **`pdf-render` is pure Rust** (pdf-interpret + skrifa + ttf-parser + image/png + vello_cpu), statically linkable into a single binary, with **no native/-sys deps**. Version used: ~v1.0.0-beta.17.
- **`rasterrocket` is REJECTED:** it is **not** pure Rust / not single-binary — it pulls `freetype-rs` (system FreeType via pkg-config) and `openjpeg-sys`; `ldd` shows it dynamically links `libfreetype.so.6`. Also its crates.io release is broken (sub-crate `rasterrocket-color` versions yanked → resolution fails; only the git workspace works). This violates the single-self-contained-static-binary / zero-runtime-system-deps requirement.
- **`mupdf` (AGPL) and `pdfium-render` (needs a pdfium binary at runtime)** remain **dropped** for the same reason.

#### 4.4.2 `pdf_render` API notes

- Render via `Pdf::new` + `InterpreterSettings` + `RenderSettings { x_scale, y_scale, bg_color }` at **scale 300/72**.
- Extract grayscale from `pixmap.data_as_u8_slice()` (premultiplied RGBA8 → luma) and feed it to OCR.
- `Pdf::new` error type is `LoadPdfError` — needs `{:?}` (no `Display` impl).

The renderer sits behind the `PdfPageRenderer` port (implemented by `PdfRenderer` in `pdf_renderer.rs`), so it stays swappable, but `pdf_render` is the **chosen** implementation.

## 5. Architecture (DDD / Clean Architecture)

### 5.1 The Dependency Rule

- **Domain** and **Application** layers must know **nothing** about the MCP framework (`rmcp`) or the document library (`anydoc` / `pdf-inspector`).
- All external concerns are reached through **ports** (traits) defined in the domain.
- **Infrastructure** implements the ports.
- **Interface/Adapter** (MCP server) translates incoming tool calls into application use-case invocations.

Dependencies point **inward** only:

```
Interface (rmcp server) → Application (use cases) → Domain (entities + ports)
                                     ↑
                          Infrastructure (adapters) implements ports
```

### 5.2 Proposed crate layout

A single crate with modules (recommended for a small project), or Cargo workspaces if modules grow. Modules map to layers:

```
mcp-documents/
├── Cargo.toml
├── src/
│   ├── main.rs                 # stdio entrypoint (Interface layer)
│   ├── lib.rs                  # library root, re-exports
│   │
│   ├── domain/                 # DOMAIN LAYER — no framework/lib imports
│   │   ├── mod.rs
│   │   ├── document.rs         # DocumentKind, PdfType, PageClassification
│   │   ├── result.rs           # ExtractionResult, ExtractedPage, MergePlan
│   │   ├── error.rs            # ExtractionError (domain error)
│   │   └── ports.rs            # DocumentExtractor, OcrProvider, PdfPageRenderer (traits)
│   │
│   ├── application/            # APPLICATION LAYER — use cases
│   │   ├── mod.rs
│   │   ├── classify_document.rs   # ClassifyDocumentUseCase
│   │   └── extract_document.rs    # ExtractDocumentUseCase (route + merge)
│   │
│   ├── infrastructure/         # INFRASTRUCTURE — adapters
│   │   ├── mod.rs
│   │   ├── anydoc_extractor.rs    # AnyDocExtractor (implements DocumentExtractor)
│   │   ├── tesseract_ocr.rs       # TesseractOcrProvider (tesseract-rs, embedded tessdata)
│   │   ├── pdf_renderer.rs        # PdfPageRenderer impl (pdf_render — pure-Rust rasterizer)
│   │   ├── file_reader.rs         # file read helpers (path → bytes)
│   │   └── error_map.rs           # ConvertError / lib errors → ExtractionError
│   │
│   └── interface/              # INTERFACE/ADAPTER — MCP server
│       ├── mod.rs
│       ├── server.rs           # #[tool_router(server_handler)] struct
│       └── tools.rs            # #[tool(...)] methods calling use cases
│
└── tests/                      # integration tests (infra adapters vs fixtures)
    ├── fixtures/
    └── ...
```

> If the repo later grows, promote each layer (or `infrastructure`) into a workspace member crate. The module boundaries above are the contract to preserve.

### 5.3 Distribution / single self-contained binary

- The server ships as **one self-contained binary** (single file).
- **OCR language data is embedded** into the binary via `tesseract-rs`'s `embed-tessdata` feature (`eng`+`rus`), so no external `tessdata` files are needed at runtime.
- **Zero runtime OCR/system packages:** the OCR (`tesseract-rs` vendored compile) and render (pure-Rust rasterizer) stack statically links into the binary; no installed packages, no `TESSDATA_PREFIX`, no environment setup.
- **Dynamic-linking caveat:** the binary is **not fully static** — it dynamically links glibc + libstdc++. The runtime base image must match the builder's glibc/libstdc++ ABI (the Dockerfile uses `debian:trixie-slim`, same Debian release as the builder). A fully static (musl) build is out of scope for Round 1.
- **No external binary is ever invoked** — nothing is shelled out.
- **Built via Docker multi-stage:** the builder compiles and pre-provisions `rus.traineddata`, and the runtime stage ships **only the binary**. Build the shipped artifact in **release mode** with `TESSERACT_EMBED_LANGUAGES=eng,rus`.

## 6. Domain Layer

### 6.1 Entities & value objects

```rust
// document.rs
pub enum DocumentKind {
    Pdf, Doc, Docx, Xls, Xlsx, Ppt, Pptx,
    Odt, Ods, Odp, Rtf, Epub, Csv, Unknown,
}
// lowercase as_str(): "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
// "odt", "ods", "odp", "rtf", "epub", "csv", "unknown"

pub enum PdfType { TextBased, Scanned, ImageBased, Mixed }

pub struct PageClassification {
    pub page_index: usize,
    pub needs_ocr: bool,
    pub confidence: f32,
}
```

```rust
// result.rs
pub struct ExtractionResult {
    pub kind: DocumentKind,
    pub pdf_type: Option<PdfType>,
    pub markdown: String,
    pub pages: Vec<ExtractedPage>,
}

pub struct ExtractedPage {
    pub page_index: usize,
    pub source: PageSource, // Local | Ocr
    pub text: String,
}
```

### 6.2 Domain errors

```rust
// error.rs
pub enum ExtractionError {
    Unsupported(DocumentKind),   // format we don't handle
    Malformed(String),           // broken / unparseable input
    Encrypted,                   // password-protected
    ResourceLimit(String),       // size / page / memory limits exceeded
    MissingPart(String),         // expected embedded asset missing
    Io(String),                  // underlying I/O failure
    OcrUnavailable(String),      // scanned page but no OCR provider configured
    Internal(String),            // unexpected / invariant violation
}
```

These are **domain-native** errors. Infrastructure maps third-party errors onto them (see §7.3).

### 6.3 Ports (traits)

```rust
// ports.rs
pub trait DocumentExtractor {
    /// Detect the document kind from content.
    fn classify(&self, source: &Source) -> Result<Classification, ExtractionError>;

    /// Extract Markdown/text from a document (text-based path).
    fn extract_text(&self, source: &Source) -> Result<ExtractionResult, ExtractionError>;
}

pub trait OcrProvider {
    /// OCR a single page image, returning extracted text.
    fn ocr_page(&self, bitmap: &PageBitmap) -> Result<String, ExtractionError>;
}

pub trait PdfPageRenderer {
    /// Render a single PDF page to a bitmap (e.g. grayscale) for OCR consumption.
    fn render_page(&self, pdf_bytes: &[u8], page_index: usize, dpi: u32)
        -> Result<PageBitmap, ExtractionError>;
}
```

The renderer and OCR provider exchange a domain value object **`PageBitmap { width: u32, height: u32, bytes_per_pixel: u8, data: Vec<u8> }`** (in `src/domain/result.rs`) that carries the **real rendered page geometry**:

```rust
// result.rs
pub struct PageBitmap {
    pub width: u32,
    pub height: u32,
    pub bytes_per_pixel: u8, // 1 = grayscale, 3 = RGB, 4 = RGBA
    pub data: Vec<u8>,       // tight, row-major
}
```

**Why `PageBitmap`:** Tesseract's `set_image` needs true `width`/`height`/`bytes_per_row`. Guessing a square page (e.g. sqrt of the byte length) misreads any **non-square** A4 scan → empty OCR. Carrying geometry as a value object fixes this; `bytes_per_row()` is derived from `width * bytes_per_pixel`.

`Source` **owns** its data — `Path(PathBuf)` | `Bytes(Vec<u8>)` (or `bytes::Bytes`) — rather than borrowing `&[u8]`, so it can be moved into `spawn_blocking`/`'static` closures (used for render + OCR) without lifetime fights. Ports stay library-agnostic. `PdfPageRenderer` keeps the rasterizer swappable behind a port.

## 7. Application Layer

Use cases orchestrate business rules; they depend only on the domain ports — **never** on `anydoc` or `rmcp`.

### 7.1 `ClassifyDocumentUseCase`

```
input: Source
  → DocumentExtractor::classify(source)
  → returns { kind, pdf_type, pages_needing_ocr, confidence }
```

### 7.2 `ExtractDocumentUseCase` (the core routing logic)

The recommended PDF flow:

```
input: Source (PDF)
  1. classify the PDF           → PdfType + pages_needing_ocr
  2. route text-based pages     → DocumentExtractor::extract_text  (local)
  3. for pages_needing_ocr      → render each page to a bitmap image
  4. OCR each rendered page     → OcrProvider::ocr_page             (per page)
  5. merge results              → combine pages into one ExtractionResult
  6. return Markdown (+ plain-text variant)
```

**Rules:**
- If the PDF is fully `TextBased`: skip OCR entirely; return the local extraction.
- If it contains scanned pages and **no** OCR provider is configured: return `ExtractionError::OcrUnavailable` (surfaced to the caller as a clear tool-level message) rather than silently dropping content.
- Page order must be preserved after merging; each page records its `source` (Local | Ocr) for transparency.

**Operational robustness (panic isolation):** document-conversion/parse calls are wrapped in panic isolation (`std::panic::catch_unwind`, per-file / per-request). A single malformed document must **never abort the whole MCP request or process** — the spike proved `pdf-extract`/`cff-parser` can panic on embedded-font PDFs. `anydoc` is the confirmed engine but is still guarded **defensively**, so a panic is translated into a domain error (e.g. `ExtractionError::Internal` / `Malformed`) and a tool-level failure instead of a crash. This isolation is implemented in the infrastructure layer around the conversion call.

**OCR concurrency model:** stdio MCP uses JSON-RPC request-ID multiplexing, so concurrent tool requests are possible in principle; `rmcp` (tokio) can serve them concurrently. The main practical win is **intra-request page-parallel OCR** (many scanned pages of one PDF in parallel) plus handling concurrent requests.

- At startup, **materialize the embedded tessdata bytes into a temp directory** (because `init_embedded("eng+rus")` fails — see §4.3.1), then initialize the Tesseract API **once** with `api.init(temp_dir, "eng+rus")?`.
- Maintain a **small pool of `try_clone()` instances**, sized to available CPU cores. Rationale: Tesseract init is expensive (~0.5–1 s), so a clone pool avoids repeated per-page init.
- Run each OCR through **`tokio::task::spawn_blocking`** — OCR is CPU/IO-blocking and not async-safe on the runtime thread.
- Merge page results in order after OCR.

> **R1 fallback:** a simpler per-request fresh init is acceptable for Round 1, but the clone pool is **recommended**.

### 7.3 Error mapping (infrastructure → domain)

Infrastructure adapters translate third-party errors into `ExtractionError`:

| `anydoc::ConvertError` / source | Domain `ExtractionError` |
|----------------------------------|--------------------------|
| `Unsupported`                   | `Unsupported(kind)`      |
| `Malformed`                     | `Malformed(...)`         |
| `Encrypted`                     | `Encrypted`              |
| `ResourceLimit`                 | `ResourceLimit(...)`     |
| `MissingPart`                   | `MissingPart(...)`       |
| `Io`                            | `Io(...)`                |
| any other / unexpected          | `Internal(...)`          |

This mapping lives in `infrastructure/error_map.rs`. It guarantees the application layer never leaks library-specific error types.

## 8. Interface / Adapter Layer (MCP server)

Built on **`rmcp`** (`modelcontextprotocol/rust-sdk`), tokio async runtime.

```bash
cargo add rmcp --features server
```

### 8.1 Server definition

```rust
use rmcp::{
    model::CallToolResult, tool, tool_router,
    transport::stdio, ServiceExt,
};

#[tool_router(server_handler)]
struct DocumentsServer { /* use cases injected here */ }

#[tool(description = "Classify a PDF, returning its type and pages needing OCR")]
async fn classify_pdf(&self, params: Parameters<ClassifyParams>) -> Result<CallToolResult, McpError> {
    // call ClassifyDocumentUseCase
}

#[tool(description = "Extract Markdown text from a PDF (text-based, with OCR for scanned pages)")]
async fn extract_pdf(&self, params: Parameters<ExtractParams>) -> Result<CallToolResult, McpError> {
    // call ExtractDocumentUseCase
}
```

stdio entrypoint:

```rust
DocumentsServer::new(deps).serve(stdio()).await?;
service.waiting().await?;
```

### 8.2 Tool surface

| Tool | Input | Output |
|------|-------|--------|
| `classify_pdf` | `path` (or `bytes`) | `{ pdf_type, pages_needing_ocr, confidence }` |
| `extract_pdf` | `path` (or `bytes`) | Markdown text of the PDF (with OCR) |
| `extract_pdf_text` | `path` (or `bytes`) | Plain-text variant |
| `classify_document` | `path` (+ optional base64 `bytes`) | `{ kind, pdf_type?, pages_needing_ocr? }` for **any** supported format |
| `extract_document` | `path` (+ optional base64 `bytes`) | Markdown text for **any** supported format (PDF uses OCR; others go straight through `anydoc`) |

Params structs derive `serde::Deserialize` + `schemars::JsonSchema`. The PDF-specific tools remain; `classify_document`/`extract_document` are the generic multi-format entry points.

### 8.3 Error handling strategy

Two distinct channels per the `rmcp` model:

1. **Tool-level errors** — `Ok(CallToolResult::error(vec![...]))`. Use for *expected* business failures the caller should see as a message: unsupported/malformed/encrypted input, OCR unavailable, resource limit. The caller receives a readable message and the tool "fails gracefully."
2. **Protocol errors** — `Err(McpError::invalid_params(...))`. Use only for *malformed tool invocations* (bad params, unknown fields, wrong types) — i.e. bugs on the client side.

**Rule of thumb:** bad *document data* → tool-level error; bad *request shape* → protocol error.

### 8.4 Dual-mode binary: MCP stdio server + CLI

The **same binary** serves both local MCP (stdio, primary) and a standalone CLI (costs nothing). Dispatch in `main.rs` parses argv: a recognized subcommand runs the CLI; otherwise it runs the MCP stdio server.

- `classify <path>` — print the classification (kind; for PDFs, `pdf_type` + pages needing OCR).
- `extract <path>` — print extracted Markdown (any supported format).
- `extract-text <path>` — print extracted plain text.
- `serve` — run the MCP stdio server (also the default when no recognized subcommand is given).

The CLI reuses the same application use cases and adapters; both modes share one binary and one set of ports.

## 9. OCR Routing Design

Neither `anydoc` nor `pdf-inspector` performs OCR — they extract text only from *text-based* PDFs. Additionally, **neither renders PDF pages to images**: the OCR engine (`tesseract-rs`) operates on a **bitmap image**, not a PDF. The OCR path therefore requires an explicit **PDF page → image renderer** step *before* the OCR provider.

The OCR engine is **vendored/compiled into the binary at build time** and the renderer is **pure Rust**, so the whole OCR+render stack statically links into the **single self-contained binary** with zero runtime system dependencies. Nothing is shelled out; no `tessdata` files exist on disk at runtime.

```
                 ┌────────────────────────────┐
   PDF ──▶ classify ──▶ PdfType + pages_needing_ocr
                 └────────────┬───────────────┘
                              │
              ┌───────────────┴───────────────┐
              ▼                               ▼
   text-based pages                     scanned/image pages
              │                               │
   DocumentExtractor (local)          PdfPageRenderer (pure-Rust
   (anydoc → pdf-inspector)           rasterizer → bitmap)
              │                               │
              │                               ▼
              │                     OcrProvider (tesseract-rs)
              │                     (init temp_dir "eng+rus",
              │                      try_clone per worker)
              │                               │
              └───────────────┬───────────────┘
                              ▼
                        Merge (ExtractDocumentUseCase)
                              ▼
                        Markdown / text
```

**Design decisions:**
- The `OcrProvider` **port** keeps OCR pluggable: `TesseractOcrProvider` (tesseract-rs) by default; an `ocrs`-based provider or a stub can be swapped in without touching application logic.
- The reference `TesseractOcrProvider` uses **`tesseract-rs`** with `embed-tessdata`: Tesseract+Leptonica compiled at build time and **`eng`+`rus` tessdata embedded**. It initializes by **materializing the embedded tessdata into a temp dir and calling `api.init(temp_dir, "eng+rus")?`** (the `init_embedded("eng+rus")` form is broken — see §4.3.1) and consumes a **`PageBitmap`** (`ocr_page(bitmap: &PageBitmap)`), returning text the merge step inserts in page order. No external process is spawned.
- **PDF page → image rendering** is a separate concern performed *before* OCR, behind the `PdfPageRenderer` port: for each `pages_needing_ocr` page, render it to a **grayscale (or grayscale-capable) bitmap at ~300 DPI** and feed it to OCR. The renderer is **`pdf_render`** (pure Rust, no native deps) so the binary stays self-contained.
- **Concurrency:** OCR is CPU-bound and runs in `spawn_blocking` workers drawn from a **pool of `try_clone()` instances** (sized to CPU cores) for **page-parallel OCR** across a PDF and across concurrent requests.
- The renderer + OCR provider are both infrastructure-only; because both are injected and isolated behind ports, swapping implementations is a pure infrastructure change.
- If OCR is unavailable for a scanned doc, fail **explicitly** (`OcrUnavailable`), never silently return partial/empty content.

## 10. Security Considerations

- **Path access:** treat `path` inputs as untrusted (MCP clients may be driven by untrusted prompts). Decide explicitly and document it: either **canonicalize + restrict to an allowlisted directory**, or **accept arbitrary-read-by-design** and state the trust boundary loudly in the tool description. Resolve symlinks and prevent traversal; do not silently allow reads outside the intended scope.
- **Size limits:** `anydoc` exposes `ResourceLimit`. Enforce a maximum input size (bytes) and, where applicable, a page cap. Exceeding limits → `ExtractionError::ResourceLimit` → tool-level error, never an OOM.
- **Bytes mode:** prefer `to_markdown_bytes` / format-from-content detection over trusting a caller-supplied extension.
- **Encrypted files:** return `Encrypted` without attempting to bypass.
- **OCR is self-contained:** OCR runs via `tesseract-rs` (vendored, embedded tessdata) in the same process — **no** external binary, and no arbitrary command execution / shell-out occurs. If a remote OCR provider were configured later, treat it as a trust boundary: validate inputs before sending and log appropriately.
- No arbitrary command execution; no network egress from the core library.

## 11. Testing & Coverage Strategy (≥ 85% line coverage)

The 85% target is a **whole-repo** line-coverage goal, achieved by testing each layer at the right level. The suite runs in **both configurations**: the default (pure-Rust, `NoopOcrProvider`) and the full `ocr-native` build (real `TesseractOcrProvider`), the latter being the coverage gate in Docker/CI.

### 11.1 Unit tests (domain + application)

- **Domain:** pure entities, value objects, error types — straightforward unit tests.
- **Application:** use cases tested against **fake/stub ports** (in-memory `FakeExtractor`, `FakeOcrProvider`, `FakePageRenderer`). This exercises classification, routing, merge order, OCR-unavailable handling, and error mapping without touching real OCR/render or the filesystem.
- Keep the heavy logic in **pure, fake-testable functions**; keep the thin OCR/render glue (`tesseract-rs`/renderer) small so most of the code path is covered by unit tests.
- Use `#[cfg(test)]` modules inside each file plus `#[tokio::test]` for async use cases.

### 11.2 Integration tests (infrastructure adapters)

- `AnyDocExtractor`, `TesseractOcrProvider`, and the `PdfPageRenderer` impl tested against **fixture documents** in `tests/fixtures/` (a text-based PDF, a scanned PDF, a mixed PDF, a malformed file, an encrypted file, an oversized input).
- Because OCR uses `tesseract-rs` with **embedded tessdata** (no runtime system dependencies), these integration tests **run normally on any machine** — they are **not** skipped for lack of a system Tesseract. (Note: they exercise the real OCR path, so the build must have embedded `eng`+`rus` tessdata.)
- These assert real conversion/OCR behavior and correct error mapping end-to-end.
- Small synthetic PDF fixtures are committed; large or generated ones are produced by a test helper to keep the repo lean.

### 11.3 Interface tests

- MCP tools tested by invoking the server handler with constructed params, asserting `CallToolResult` shape and error classification. Avoid requiring a live MCP client.

### 11.4 Coverage measurement

```bash
# install once
cargo install cargo-llvm-cov

# measure
cargo llvm-cov --html           # writes target/coverage/index.html
cargo llvm-cov --fail-under-lines 85   # CI gate
```

### 11.5 Testing tools

- `cargo llvm-cov` — coverage (the CI gate for the 85% line target).
- `#[tokio::test]` — async use-case tests.
- Fake/stub ports (in-memory implementations of `DocumentExtractor` / `OcrProvider`) — application-layer isolation.

## 12. CI

A minimal CI pipeline should run on every PR/merge to the default branch. The **full OCR binary is built in a Docker multi-stage build** (builder pre-provisions `rus.traineddata` + sets `TESSERACT_EMBED_LANGUAGES=eng,rus` + `--features ocr-native`); the runtime and the coverage gate need **no system Tesseract**. The builder needs a C++ compiler + CMake + internet (to compile/download vendored Tesseract/Leptonica + tessdata) — CI does **not** install tesseract at runtime.

1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test`
4. `cargo llvm-cov --fail-under-lines 85`

The **85% line-coverage gate runs on the Docker/CI build with `ocr-native` ON, on a bare runner** (no runtime OCR packages). (Note: the vendored C++ build is slow on first compile — cache the compiled libraries / Docker builder layers between runs.)

Artifacts: coverage report; fail the build if the 85% line gate is not met.

## 13. Dependencies (initial)

```bash
cargo add rmcp --features server,transport-io
cargo add anydoc
cargo add pdf_render           # pure-Rust PDF rasterizer (confirmed by spike; ~v1.0.0-beta.17)
cargo add tokio --features full
cargo add serde --features derive
cargo add schemars
# optional, gated behind the `ocr-native` feature (default OFF):
cargo add tesseract-rs --features embed-tessdata --optional   # vendored OCR, embedded eng+rus tessdata
# dev
cargo add --dev tokio-test
```

**Feature gate:** `tesseract-rs` is an **optional dependency** enabled by the `ocr-native` feature (default OFF). The default build is fully pure-Rust and needs **no C++ toolchain**. The full OCR binary is built with `--features ocr-native` in Docker/CI.

**No runtime system prerequisites.** Runtime has zero OS packages: no `libtesseract`, no `libleptonica`, no `tessdata` files, no `TESSDATA_PREFIX`. **Build-time requirements for `ocr-native`** (NOT runtime, NOT default build): a C++ compiler (`gcc`/`clang`), **CMake**, and **internet access** (to download Tesseract/Leptonica sources + training data). The crate caches compiled libraries for faster subsequent builds.

**Build env var (required for `ocr-native`):** the build must set `TESSERACT_EMBED_LANGUAGES=eng,rus` (default is `eng,tur` — Russian is **not** included by default) and **pre-provision `rus.traineddata`** into the tesseract-rs tessdata cache dir before building (the crate only auto-downloads `eng`/`tur`). Use the provided **Docker multi-stage build** / `make build` so this is not forgotten. See §4.3.1/§4.3.4.

> `pdf-inspector` is **not** added directly; it comes transitively via `anydoc` as the PDF engine. Add it explicitly only if we adopt the fallback path (§4.2). The renderer is **`pdf_render`** (see §4.4). If `tesseract-rs` accuracy is insufficient, `ocrs` (embedded models) remains the alternative (see §4.3.2).

## 14. Roadmap

| Round | Scope | Notes |
|-------|-------|-------|
| **R1 (done)** | PDF classification + text extraction + OCR routing (pure-Rust PDF→image renderer + `tesseract-rs` OCR with embedded eng+rus tessdata); single self-contained binary; 85% coverage | `anydoc` + `rmcp` + `tesseract-rs` (embed-tessdata) + pure-Rust renderer |
| **R2 (done)** | MS Office (Word/Excel/PowerPoint) + ODF/RTF/EPUB/CSV via `anydoc`; generic `classify_document`/`extract_document`; non-PDF routing (no OCR) | Wider `DocumentKind` + generic tools; same `DocumentExtractor` port, **no new library** |
| **R3 (candidate)** | OCR quality, concurrency/backpressure, large-file streaming, optional HTTP transport | Depends on feedback |

## 15. Open Questions

- **Renderer:** **DECIDED** — `pdf_render` is the chosen PDF page → image renderer (see §4.4); `rasterrocket` is rejected (native FreeType/OpenJPEG + broken crates.io), and `mupdf`/`pdfium-render` remain dropped. No longer open.
- **`rus` embedding + Cyrillic accuracy:** **VERIFIED working** — the Docker build (`--features ocr-native`) embeds `eng`+`rus`, the temp-dir `init` workaround works end-to-end, and a real scanned Russian receipt PDF produced correct Cyrillic OCR (~1356 chars). The `init_embedded("eng+rus")` gotcha remains documented (§4.3.1). OCR quality still depends on scan quality; the **`tessdata_best` vs `fast`** size/accuracy tradeoff remains (see below).
- **Binary size / tessdata variant:** **DECIDED — ship `tessdata_best`** for OCR accuracy. The Dockerfile provisions `tessdata_best` (eng+rus ≈ 30.7 MB, ~22 MB over `fast`); the resulting single binary is ~50–72 MB, which is acceptable. `tessdata_fast` remains the smaller fallback (≈ 8 MB eng+rus) if binary size matters later (see §4.3.1).
- **OCR concurrency:** confirm the clone-pool sizing (cores) and whether the R1 per-request-init fallback suffices vs the recommended pool (see §7.2).
- **OCR image preparation:** exact page→image parameters (resolution ~300 DPI, grayscale via `pixmap.data_as_u8_slice()` → luma) — validated by the renderer spike.
- **CLI vs MCP-only:** **DECIDED** — the binary ships **both**: MCP stdio server (default) and a standalone CLI (`classify`/`extract`/`extract-text`/`serve`), dispatched in `main.rs` (see §8.4). No longer open.
- **`extract_document` / `classify_document` exposure:** **DECIDED** — the generic tools are shipped alongside the PDF-specific ones (§8.2). No longer open.
- **`.xls`/`.xlsb` labeling:** **known limitation** — legacy binary `.doc`/`.ppt`/`.xls` extract correctly (anydoc/calamine), but `.xls` is labeled `Xlsx` because Excel formats are not distinguishable from bytes alone (§3.2).
- **Byte vs path inputs:** whether all tools accept both `path` and `bytes`, or only one, for the initial API surface. (Currently the generic tools accept `path` + optional base64 `bytes`.)
- **Remote OCR:** whether a network OCR provider is required for Round 1 or if the pure-Rust engine (and stub) is sufficient.
- **Crate layout:** single-crate modules (default here) vs Cargo workspace — finalize when the codebase grows.
