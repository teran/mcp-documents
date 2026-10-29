# Technical Specification — `mcp-documents`

> An MCP (Model Context Protocol) server that extracts clean text and Markdown from documents.

- **Language:** Rust
- **Runtime:** `tokio` (async)
- **Architecture:** DDD / Clean Architecture (ports & adapters)
- **Status:** Per-format tool surface — PDF text extraction (incl. OCR for scanned PDFs) **plus** MS Office / RTF / CSV, with a dedicated adapter per format.

---

## 1. Goals

The server extracts clean text/Markdown from a broad set of document formats. The system must be able to:

1. **Classify** a document (PDF or any supported format) — for PDFs, identify `TextBased` / `Scanned` / `ImageBased` / `Mixed` **and** which pages require OCR, per page.
2. **Extract** text/Markdown per format: PDFs per-page (with OCR for scanned pages), spreadsheets per-sheet/row-range, presentations per-slide, CSV streaming, and word-processor formats by word range.
3. **OCR** scanned / image-only **PDF** pages via a pluggable OCR provider and **merge** the results back into a coherent document, interleaving text and OCR pages in page order (non-PDF formats go straight through their adapter, no OCR).
4. Expose these capabilities to MCP clients (e.g. Claude Code, opencode) over **stdio** as typed tools, in a **per-format** surface with Markdown-only output.
5. Maintain a **line-coverage target of ≥ 95% on the default build** (≥ 85% on the `ocr-native` build).

PDF+OCR was the founding capability; multi-format support is now a natural extension with a dedicated adapter per format.

## 2. Non-Goals

- ❌ No editing, conversion *to* PDF, or document generation.
- ❌ No rendering / thumbnailing of pages (except the internal page→bitmap step needed for OCR).
- ❌ No network service / HTTP transport (stdio only for now).
- ❌ **No external OCR process or system OCR at runtime** — OCR runs via `tesseract-rs` with its `embed-tessdata` feature (Tesseract+Leptonica vendored and compiled at build time; language data **embedded in the binary**). Nothing is shelled out; no system `tesseract` / `libtesseract` / `libleptonica` / `tessdata` is required at runtime.
- ❌ No runtime system dependencies — the shipped artifact is a **single self-contained binary** (embedded OCR data; dynamically links glibc/libstdc++ only) with **zero** installed packages required.
- ❌ No persistence / caching of extracted documents (the only on-disk cache is the materialized tessdata for OCR, a regenerable cache).

## 3. Scope

### 3.1 PDF + OCR (foundation, implemented)

- PDF classification per page (`classify_document` for PDFs → PDF type + pages needing OCR; `pdf_info`).
- PDF → Markdown extraction, **per page** (`pdf_extract_markdown`), with a `page_from`/`page_to` range.
- OCR routing for scanned/image pages via the `OcrProvider` port + reference `TesseractOcrProvider` adapter (`tesseract-rs` with **embedded tessdata**, English + Russian).
- Real per-page PDF text + classification via **`pdf-inspector`** (directly, not through `anydoc`).
- MCP stdio server (`rmcp`).

### 3.2 Multi-format (R2, implemented)

- **Per-format tool surface:** for each of `pdf`, `doc`, `docx`, `rtf`, `xls`, `xlsx`, `ppt`, `pptx`, `csv` there is an `_info` tool (metadata) and an `_extract_markdown` tool (Markdown output), plus the single generic `classify_document`.
- **Dedicated adapters per format** (no single engine for everything):
  - **PDF** — `pdf-inspector` per-page text + classification; `pdf_render` rasterizer + OCR for scanned pages.
  - **CSV** — streaming `csv` adapter (memory < file size), `row_from`/`row_to`.
  - **xls / xlsx** — `calamine` sheets + row ranges; `.xls` vs `.xlsx` distinguished by **magic bytes**.
  - **ppt / pptx** — `.pptx` via `zip` + `quick-xml` slide parts (slide ranges); legacy binary `.ppt` via `anydoc` whole-document (no slide range).
  - **doc / docx / rtf** — word/char counting + `word_from`/`word_to` word ranges via `anydoc`, with a `MAX_RETURN_CHARS` safety net.
- **Routing:** PDF keeps the per-page classify → render → OCR → merge path; **non-PDF formats go straight through their dedicated adapter (no OCR)**.
- **Known limitation:** legacy binary `.ppt` is a whole-document unit (no per-slide addressing); ODF (`odt`/`ods`/`odp`) and EPUB have no dedicated `_info`/`_extract_markdown` tools (only classification + CLI whole-document fallback via `anydoc`).

## 4. Library Choice

### 4.1 PDF engine: `pdf-inspector` (used directly)

For PDFs we use **`pdf-inspector` directly** rather than through `anydoc`, because we need **real per-page** text and per-page classification:

- `detect_pdf_type_mem_with_config` with `ScanStrategy::Full` classifies **every page**, so `TextBased` / `Scanned` / `ImageBased` / `Mixed` are accurate and the exact `pages_needing_ocr` list is known (enabling `page_from`/`page_to` ranges).
- `extract_text_with_positions_mem_pages` returns position-tagged text items grouped into per-page text, enabling true **Mixed**-PDF merging (text pages and OCR pages interleaved in page order).
- It is wrapped in panic isolation and its errors are mapped onto domain errors in the infrastructure layer.

`anydoc` remains the engine for the **non-PDF** paths (word-processor word counting, legacy binary `.ppt`, and the CLI/ODF/EPUB fallback).

#### 4.1.1 Why `anydoc` for the non-PDF fallback

`anydoc` was validated empirically against alternatives (7/7 real files converted, including legacy `.doc`/`.xls`/`.ppt`); `markitdown-rs` is rejected as primary (quality + robustness + legacy gaps). It remains the whole-document fallback for word/legacy formats and ODF/EPUB.

### 4.2 OCR engine choice: `tesseract-rs` (primary, feature-gated)

The MCP server must ship as a **single self-contained binary with ZERO runtime system dependencies** — no installed Tesseract, no `libtesseract`/`libleptonica`, no `tessdata` files on disk, and no external binary invoked. **`tesseract-rs`** (crate `tesseract-rs`, cafercangundogdu, v0.4.0, MIT) with its **`embed-tessdata`** feature satisfies this: Tesseract + Leptonica are **vendored and compiled from source at build time**, and the language data is **embedded into the binary**, so the runtime needs no system OCR and no `tessdata` directory.

**Feature gate — `ocr-native` (default OFF):** to keep local `cargo build`/`cargo test` **pure-Rust** (no C++ compiler / CMake / `rus.traineddata` needed), the heavy OCR dependency `tesseract-rs` is an **optional dependency** gated behind the cargo feature **`ocr-native`** (default OFF).

- **Default build (pure Rust):** `rmcp`, `anydoc`, `pdf-inspector`, `pdf-render`, `csv`, `calamine`, `zip`, `quick-xml`, `tokio`, `serde`, `schemars` — compiles and tests locally with **no C++ toolchain**.
- **With `--features ocr-native`:** enables `tesseract-rs` (`embed-tessdata`). The `TesseractOcrProvider` adapter is `#[cfg(feature="ocr-native")]`; when the feature is off, a `NoopOcrProvider` returns `ExtractionError::OcrUnavailable`, so the OCR routing compiles and behaves correctly in both configurations.
- **Full OCR binary** is built in Docker/CI with the feature on (the Dockerfile sets `TESSERACT_EMBED_LANGUAGES=eng,rus`, pre-provisions `rus.traineddata`, and builds with `--features ocr-native`). See §4.2.4 / §12.

#### 4.2.1 Primary: `tesseract-rs` (embed-tessdata)

- **Crate:** `tesseract-rs = { version = "0.4.0", features = ["embed-tessdata"] }`.
- **Engine:** compiles Tesseract + Leptonica from source at **build time** (vendored). Runtime needs **no** system `tesseract` / `libtesseract` / `libleptonica` / `tessdata` — stays a single self-contained binary.
- **Languages:** embedded at build time via the env var `TESSERACT_EMBED_LANGUAGES=eng,rus cargo build`. The **default is English + Turkish (`eng, tur`) — Russian is NOT included by default**, so the build **MUST set `TESSERACT_EMBED_LANGUAGES=eng,rus`** to support the Russian documents this server targets.
- **Runtime init — gotcha:** `init_embedded("eng+rus")` does **NOT** work: the embedded tessdata map is keyed **per single language**, so a `"eng+rus"` lookup returns `None` → `InitError`. **Workaround (still zero external runtime deps):** at startup, materialize the embedded bytes (via `get_embedded_tessdata` / `embedded_languages`) into a **stable per-user cache dir**, then call `init(cache_dir, "eng+rus")`. The bytes come from the embedded binary, not disk/network, so the single-binary / zero-runtime-dep property is preserved.
- **Startup cache (fast path):** the `.traineddata` files are treated as a **cache** — written **once** to a stable per-user dir (`$XDG_CACHE_HOME/mcp-documents/tessdata` → `~/.cache/mcp-documents/tessdata` → temp-dir fallback). On subsequent startups the provider only **size-checks** existing files (`metadata().len()` stat), rewriting only if a file is missing or its size differs. This makes server startup fast after the first run. Writes are **idempotent + self-healing** via the size check, so no lock is needed (safe under concurrent starts/tests).
- **Concurrency:** `api.try_clone()` clones the API for **parallel OCR** — each worker OCRs its own cloned instance (see §9).
- **tessdata variant & size:** the crate's `build.rs` downloads **`tessdata_best`** (hardcoded `.../tessdata_best/...` base URL) — the accurate LSTM variant. Measured sizes: `best` eng ≈ 15.4 MB / rus ≈ 15.3 MB (eng+rus ≈ 30.7 MB), vs `fast` eng ≈ 4.1 MB / rus ≈ 3.9 MB (eng+rus ≈ 8 MB). We ship **`tessdata_best`** for OCR accuracy (the Dockerfile provisions it); `tessdata_fast` is the smaller alternative (≈ 22 MB less) if binary size ever becomes critical.
- **Gotcha — `rus` is NOT auto-downloaded:** `build.rs` hardcodes the download list to `["eng", "tur"]`. `TESSERACT_EMBED_LANGUAGES` only selects which **already-downloaded** files to embed; it does **not** trigger downloads. Embedding `rus` therefore **silently fails** ("Language rus not found") unless `rus.traineddata` is physically present in the tesseract-rs tessdata cache dir **before** the build runs.

#### 4.2.2 What we do NOT use

- **`leptess`** and the **`tesseract`** crate are **dropped** — they require system-installed `libtesseract` + `tessdata` at runtime, which violates the zero-runtime-dependency constraint.
- **No external `tesseract` binary** is ever invoked (no shelling out).
- `ocrs` (pure-Rust ML OCR) remains a one-line alternative if `tesseract-rs` accuracy proves insufficient, but is not the chosen primary.

The `OcrProvider` port keeps this decision swappable: swapping `TesseractOcrProvider` for another provider is a pure infrastructure-layer change.

#### 4.2.3 Build tooling (Docker multi-stage)

The env var and the `rus.traineddata` provisioning are easy to get wrong, so the build is performed in a **Docker multi-stage build**.

- **Builder stage:** a C++ compiler + CMake image; **pre-provision `rus.traineddata`** into the tesseract-rs tessdata cache dir (macOS: `~/Library/Application Support/tesseract-rs/tessdata/`, Linux: `~/.tesseract-rs/tessdata/`) — download it from a **pinned tessdata source** into that cache dir **before** `cargo build`; set `TESSERACT_EMBED_LANGUAGES=eng,rus`; run `cargo build --release --features ocr-native`.
- **`rus.traineddata` download source:** GitHub raw can be flaky for downloading `rus.traineddata` at build time — the Docker builder should use a **mirror (e.g. jsdelivr)** or a **host copy** of the file instead.
- **Runtime stage:** uses **`gcr.io/distroless/cc-debian13:nonroot`** (a base that **matches the builder's glibc/libstdc++ ABI**); **copy only the single self-contained binary**. Zero OCR/system packages are added, nothing shelled out, embedded `eng`+`rus` tessdata. The server runs as **`nobody` (UID 65534)** with a writable **`/cache`** directory (`XDG_CACHE_HOME=/cache`) for the materialized tessdata.
- **ABI note:** the binary is **dynamically linked against glibc + libstdc++** (Tesseract/Leptonica are statically linked, but glibc/libstdc++ are dynamic). A Debian-13/trixie-built binary needs `GLIBC_2.38` / `GLIBCXX_3.4.31`, which `cc-debian12` (bookworm, glibc 2.36) lacks — hence the runtime base **must match the builder's Debian release** (`cc-debian13`). The `cc` variant (not `base`/`static`) is required because it bundles libstdc++ alongside glibc. A fully static binary would require a musl build, which is **out of scope for R1**.

### 4.3 PDF page → image renderer choice: `pdf_render` (finalized)

The OCR engine consumes **bitmap images**, not PDFs, so the pipeline needs a PDF page → image renderer that is **pure Rust** (to keep the single self-contained binary). This was settled by a **Docker-based spike** (base `rust:1.97.1-trixie`) comparing `pdf_render` vs `rasterrocket`, both feeding `tesseract-rs` (eng+rus) on 5 real Russian scanned PDFs at 300 DPI.

#### 4.3.1 Result: `pdf_render` confirmed

- Both succeeded on all PDFs and Cyrillic came through in both, but **`pdf_render` was cleaner**: it read a small-font receipt correctly that `rasterrocket` garbled (`Чек 1`: pdf_render read "Яндекс … Итого 97.00 Р", rasterrocket produced garbage), and it kept Latin/Cyrillic word shapes intact where `rasterrocket` mangled them.
- **`pdf-render` is pure Rust** (pdf-interpret + skrifa + ttf-parser + image/png + vello_cpu), statically linkable into a single binary, with **no native/-sys deps**. Version used: ~v1.0.0-beta.17.
- **`rasterrocket` is REJECTED:** it is **not** pure Rust / not single-binary — it pulls `freetype-rs` (system FreeType via pkg-config) and `openjpeg-sys`; `ldd` shows it dynamically links `libfreetype.so.6`. Also its crates.io release is broken (sub-crate `rasterrocket-color` versions yanked → resolution fails; only the git workspace works). This violates the single-self-contained-static-binary / zero-runtime-system-deps requirement.
- **`mupdf` (AGPL) and `pdfium-render` (needs a pdfium binary at runtime)** remain **dropped** for the same reason.

#### 4.3.2 `pdf_render` API notes

- Render via `Pdf::new` + `InterpreterSettings` + `RenderSettings { x_scale, y_scale, bg_color }` at **scale 300/72**.
- Extract grayscale from `pixmap.data_as_u8_slice()` (premultiplied RGBA8 → luma) and feed it to OCR.
- `Pdf::new` error type is `LoadPdfError` — needs `{:?}` (no `Display` impl).
- A render **cap** (`MAX_RENDER_PIXELS` = 100M px, `MAX_RENDER_SIDE` = 15k px) guards against a giant pixmap allocation from a pathological page/DPI.

The renderer sits behind the `PdfPageRenderer` port (implemented by `PdfRenderer` in `pdf_renderer.rs`), so it stays swappable, but `pdf_render` is the **chosen** implementation.

## 5. Architecture (DDD / Clean Architecture)

### 5.1 The Dependency Rule

- **Domain** and **Application** layers must know **nothing** about the MCP framework (`rmcp`) or the document libraries (`anydoc` / `pdf-inspector` / `pdf-render` / `csv` / `calamine` / `zip` / `quick-xml` / `tesseract-rs`).
- All external concerns are reached through **ports** (traits) defined in the domain.
- **Infrastructure** implements the ports.
- **Interface/Adapter** (MCP server) translates incoming tool calls into application use-case invocations.

**Exception (explicitly allowed):** the application layer is **async** and may use `tokio::task` (e.g. `spawn_blocking`, `JoinSet`, `Semaphore`) for the PDF OCR concurrency. The ban on business libraries does **not** extend to `tokio` task primitives.

Dependencies point **inward** only:

```
Interface (rmcp server) → Application (use cases) → Domain (entities + ports)
                                     ↑
                          Infrastructure (adapters) implements ports
```

### 5.2 Crate layout

Modules map to layers:

```
mcp-documents/
├── Cargo.toml
├── src/
│   ├── main.rs                 # stdio entrypoint / CLI dispatch (Interface layer)
│   ├── lib.rs                  # library root
│   │
│   ├── domain/                 # DOMAIN LAYER — no framework/lib imports
│   │   ├── mod.rs
│   │   ├── document.rs         # DocumentKind, PdfType, PageClassification, Classification
│   │   ├── meta.rs             # PdfInfo, TextDocumentInfo, SheetInfo, SpreadsheetInfo, PresentationInfo, CsvInfo
│   │   ├── ranges.rs           # resolve_range (inclusive-from/exclusive-to, 0-based)
│   │   ├── result.rs           # ExtractionResult, ExtractedPage, PageSource, PageBitmap, Source, merge_pages
│   │   ├── error.rs            # ExtractionError (domain error)
│   │   └── ports.rs            # DocumentExtractor, PdfTextSource, TextDocumentReader, SpreadsheetReader, PresentationReader, CsvReader, OcrProvider, PdfPageRenderer
│   │
│   ├── application/            # APPLICATION LAYER — use cases (may use tokio::task)
│   │   ├── mod.rs
│   │   ├── classify_document.rs   # ClassifyDocumentUseCase
│   │   ├── pdf.rs                # PdfUseCase (per-page + OCR concurrency/cap)
│   │   ├── text_doc.rs           # TextDocUseCase (doc/docx/rtf word ranges)
│   │   ├── spreadsheet.rs        # SpreadsheetUseCase (xls/xlsx sheets)
│   │   ├── presentation.rs       # PresentationUseCase (ppt/pptx slides)
│   │   └── csv.rs                # CsvUseCase (streaming)
│   │
│   ├── infrastructure/         # INFRASTRUCTURE — adapters
│   │   ├── mod.rs
│   │   ├── anydoc_extractor.rs    # AnyDocExtractor (DocumentExtractor: classify + anydoc fallback)
│   │   ├── pdf_text.rs            # PdfTextSourceAdapter (pdf-inspector per-page)
│   │   ├── pdf_renderer.rs        # PdfRenderer (pdf_render — pure-Rust rasterizer)
│   │   ├── text_doc.rs            # AnyDocTextReader (doc/docx/rtf word/char counts)
│   │   ├── spreadsheet.rs         # CalamineSpreadsheetReader (xls/xlsx)
│   │   ├── presentation.rs        # AnyDocPresentationReader (pptx via zip/quick-xml; .ppt via anydoc)
│   │   ├── csv.rs                 # CsvStreamReader (streaming)
│   │   ├── table.rs               # render_table (Markdown table for CSV/xls/xlsx)
│   │   ├── file_reader.rs         # file read + input/ALLOW_ROOT_DIR validation
│   │   ├── error_map.rs           # library errors → ExtractionError
│   │   ├── noop_ocr.rs            # NoopOcrProvider (OCR off)
│   │   └── tesseract_ocr.rs       # TesseractOcrProvider (ocr-native only)
│   │
│   └── interface/              # INTERFACE/ADAPTER — MCP server + CLI
│       ├── mod.rs
│       ├── server.rs           # #[tool_router] struct, per-format #[tool] methods
│       ├── tools.rs            # param structs, source construction/validation, result helpers
│       └── cli.rs              # classify / extract / serve subcommands
│
└── tests/                      # integration tests
    ├── fixtures/               # simple_text.docx / simple_text.pdf / simple_text.xlsx
    ├── format_adapters.rs
    ├── multi_format.rs
    ├── pdf_extraction.rs
    └── ocr_native.rs
```

> If the repo later grows, promote each layer (or `infrastructure`) into a workspace member crate. The module boundaries above are the contract to preserve.

### 5.3 Distribution / single self-contained binary

- The server ships as **one self-contained binary** (single file).
- **OCR language data is embedded** into the binary via `tesseract-rs`'s `embed-tessdata` feature (`eng`+`rus`), so no external `tessdata` files are needed at runtime.
- **Zero runtime OCR/system packages:** the OCR (`tesseract-rs` vendored compile) and render (pure-Rust rasterizer) stack statically links into the binary; no installed packages, no `TESSDATA_PREFIX`, no environment setup.
- **Dynamic-linking caveat:** the binary is **not fully static** — it dynamically links glibc + libstdc++. The runtime base image must match the builder's glibc/libstdc++ ABI (the Dockerfile uses `gcr.io/distroless/cc-debian13:nonroot`, Debian 13/trixie — same release as the builder). A fully static (musl) build is out of scope for Round 1.
- **No external binary is ever invoked** — nothing is shelled out.
- **Built via Docker multi-stage:** the builder compiles and pre-provisions `rus.traineddata`, and the runtime stage ships **only the binary**, running as `nobody` (UID 65534) with a writable `/cache`.

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

pub struct Classification {
    pub kind: DocumentKind,
    pub pdf_type: Option<PdfType>,
    pub pages_needing_ocr: Vec<PageClassification>,
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

```rust
// meta.rs — returned by *_info tools
pub struct PdfInfo          { page_count, pdf_type, pages_needing_ocr }
pub struct TextDocumentInfo { word_count, char_count }        // doc/docx/rtf
pub struct SheetInfo        { name, rows: Option<usize>, cols: Option<usize> }
pub struct SpreadsheetInfo  { sheets: Vec<SheetInfo> }        // xls/xlsx
pub struct PresentationInfo { slide_count }                    // ppt/pptx (.ppt → 1)
pub struct CsvInfo          { row_count, column_count, header: Vec<String> }
```

### 6.2 Domain errors

```rust
// error.rs
pub enum ExtractionError {
    Unsupported(DocumentKind),   // format we don't handle / wrong kind for a tool
    Malformed(String),           // broken / unparseable input
    Encrypted,                   // password-protected
    ResourceLimit(String),       // size / page / return-char / render limits exceeded
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
pub trait DocumentExtractor: Send + Sync {
    fn classify(&self, source: &Source) -> Result<Classification, ExtractionError>;
    /// Whole-document Markdown for a non-PDF format via `anydoc` (fallback).
    fn extract_text(&self, source: &Source) -> Result<ExtractionResult, ExtractionError>;
}

pub trait PdfTextSource: Send + Sync {
    fn pdf_info(&self, source: &Source) -> Result<PdfInfo, ExtractionError>;
    fn extract_pages(&self, source: &Source, pages: &[usize])
        -> Result<Vec<ExtractedPage>, ExtractionError>;
}

pub trait TextDocumentReader: Send + Sync {          // doc/docx/rtf
    fn text_info(&self, source: &Source) -> Result<TextDocumentInfo, ExtractionError>;
    fn extract_text(&self, source: &Source) -> Result<String, ExtractionError>;
}

pub trait SpreadsheetReader: Send + Sync {           // xls/xlsx
    fn spreadsheet_info(&self, source: &Source) -> Result<SpreadsheetInfo, ExtractionError>;
    fn extract_sheet(&self, source: &Source, sheet: Option<&str>, row_range: Option<Range<usize>>)
        -> Result<String, ExtractionError>;
}

pub trait PresentationReader: Send + Sync {          // ppt/pptx
    fn presentation_info(&self, source: &Source) -> Result<PresentationInfo, ExtractionError>;
    fn extract_presentation(&self, source: &Source, slide_range: Option<Range<usize>>)
        -> Result<String, ExtractionError>;
}

pub trait CsvReader: Send + Sync {                   // csv
    fn csv_info(&self, source: &Source) -> Result<CsvInfo, ExtractionError>;
    fn extract_csv(&self, source: &Source, row_range: Option<Range<usize>>)
        -> Result<String, ExtractionError>;
}

pub trait OcrProvider: Send + Sync {
    fn ocr_page(&self, bitmap: &PageBitmap) -> Result<String, ExtractionError>;
}

pub trait PdfPageRenderer: Send + Sync {
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

`Source` **owns** its data — `Path(PathBuf)` | `Bytes(Vec<u8>)` — rather than borrowing `&[u8]`, so it can be moved into `spawn_blocking`/`'static` closures (used for render + OCR) without lifetime fights. Ports stay library-agnostic.

## 7. Application Layer

Use cases orchestrate business rules; they depend **only** on the domain ports — **never** on `anydoc`/`rmcp`/`pdf-inspector`/etc. The application layer **may** use `tokio::task` primitives (it is async).

### 7.1 `ClassifyDocumentUseCase`

```
input: Source
  → DocumentExtractor::classify(source)
  → returns { kind, pdf_type, pages_needing_ocr }
```

### 7.2 `PdfUseCase` (per-page, the core OCR routing)

```
input: Source (PDF), page_range?
  1. require the source is a PDF          (else Unsupported)
  2. pdf_info                            → page_count + pages_needing_ocr
  3. resolve the page range              (resolve_range, 0-based)
  4. split range pages into text vs OCR  (per-page classification)
  5. cap OCR pages                       (OCR_MAX_PAGES → ResourceLimit)
  6. native text pages                   → PdfTextSource::extract_pages
  7. scanned pages                       → render + OCR, in parallel bounded by OCR_CONCURRENCY
  8. merge pages in page order           → ExtractionResult (from_pages)
```

**Rules:**
- Fully `TextBased` PDF → local extraction only, no OCR.
- Fully `Scanned`/image-only → every page rendered + OCR'd.
- If scanned pages exist and **no** OCR provider is configured → `ExtractionError::OcrUnavailable` (a clear tool-level message), never silent/partial output.
- **Mixed PDFs** interleave text and OCR pages **in page order** — per-page interleaving is now **resolved** (both `PdfTextSource` and the OCR path produce per-page `ExtractedPage`s, merged by page index).

**Operational robustness (panic isolation):** document-conversion/parse calls are wrapped in `catch_unwind` (per-file / per-request) in the infrastructure layer, so a malformed document becomes a domain error / tool-level failure instead of a crash.

**OCR concurrency (real, bounded):**
- At startup, **materialize the embedded tessdata bytes into a stable per-user cache dir** (because `init_embedded("eng+rus")` fails — see §4.2.1), then initialize the Tesseract API **once** with `api.init(cache_dir, "eng+rus")?`.
- Within a request, scanned pages are OCR'd **in parallel**, bounded by a **semaphore sized to `OCR_CONCURRENCY`** (default = CPU cores), using a **`tokio::task::JoinSet`** of `spawn_blocking` workers. Each worker renders (via `PdfPageRenderer`) then OCRs its page using a **`try_clone()`** of the initialized API (per-page clone avoids repeated ~0.5–1 s init).
- The number of OCR'd pages per request is **capped by `OCR_MAX_PAGES`** (default 300) → `ResourceLimit`.
- Render + OCR run through **`tokio::task::spawn_blocking`** — they are CPU-bound and not async-safe on the runtime thread.

### 7.3 Error mapping (infrastructure → domain)

Infrastructure adapters translate third-party errors into `ExtractionError` (in `infrastructure/error_map.rs`):

| Library error | Domain `ExtractionError` |
|---------------|--------------------------|
| `anydoc::ConvertError::Unsupported` | `Unsupported(kind)` |
| `anydoc::ConvertError::Malformed` | `Malformed(...)` |
| `anydoc::ConvertError::Encrypted` | `Encrypted` |
| `anydoc::ConvertError::ResourceLimit` | `ResourceLimit(...)` |
| `anydoc::ConvertError::MissingPart` | `MissingPart(...)` |
| `anydoc::ConvertError::Io` | `Io(...)` |
| `pdf_render::LoadPdfError::Decryption` | `Encrypted` |
| `pdf_render::LoadPdfError::Invalid` | `Malformed(...)` |
| `pdf_render::LoadPdfError::TooLarge` | `ResourceLimit(...)` |
| `pdf_inspector::PdfError::Encrypted` | `Encrypted` |
| `pdf_inspector::PdfError::InvalidStructure`/`Parse` | `Malformed(...)` |
| `pdf_inspector::PdfError::NotAPdf` | `Unsupported(...)` |
| `calamine::Error` | `Malformed(...)` / `Io(...)` |
| `zip::result::ZipError::FileNotFound` | `MissingPart(...)` |
| `zip::result::ZipError` (other) | `Malformed(...)` |

This mapping guarantees the application/interface layers never leak library-specific error types.

## 8. Interface / Adapter Layer (MCP server)

Built on **`rmcp`** (`modelcontextprotocol/rust-sdk`), tokio async runtime.

### 8.1 Server definition

```rust
use rmcp::{model::CallToolResult, tool, tool_router, transport::stdio, ServiceExt};

#[tool_router(router = tool_router)]
struct DocumentsServer { /* use cases injected here */ }

#[tool(description = "Detect the kind of any supported document (and for PDFs the type + pages needing OCR)")]
async fn classify_document(&self, Parameters(params): Parameters<ClassifyParams>) -> Result<CallToolResult, ErrorData> { ... }
```

stdio entrypoint:

```rust
server.serve(stdio()).await?;
service.waiting().await?;
```

### 8.2 Tool surface

The surface is **per-format**: one generic classifier plus, for each supported format, `_info` (metadata as JSON) and `_extract_markdown` (Markdown as text). **All** tools accept `path` + optional base64 `bytes`. Output is **Markdown only** (no plain-text tools).

| Tool | Input (common) | Output |
|------|----------------|--------|
| `classify_document` | `path` / `bytes` | `{ kind, pdf_type?, pages_needing_ocr[] }` for **any** supported format |
| `pdf_info` | `path` / `bytes` | `{ kind, page_count, pdf_type, pages_needing_ocr[] }` |
| `pdf_extract_markdown` | `path` / `bytes`, `page_from?`, `page_to?` | Markdown (OCR for scanned pages) |
| `doc_info` / `doc_extract_markdown` | `path` / `bytes` (+ `word_from?`, `word_to?`) | `{ kind, word_count, char_count }` / Markdown |
| `docx_info` / `docx_extract_markdown` | `path` / `bytes` (+ `word_from?`, `word_to?`) | `{ kind, word_count, char_count }` / Markdown |
| `rtf_info` / `rtf_extract_markdown` | `path` / `bytes` (+ `word_from?`, `word_to?`) | `{ kind, word_count, char_count }` / Markdown |
| `xls_info` / `xls_extract_markdown` | `path` / `bytes` (+ `sheet?`, `row_from?`, `row_to?`) | `{ kind, sheets[] }` / Markdown table |
| `xlsx_info` / `xlsx_extract_markdown` | `path` / `bytes` (+ `sheet?`, `row_from?`, `row_to?`) | `{ kind, sheets[] }` / Markdown table |
| `ppt_info` / `ppt_extract_markdown` | `path` / `bytes` (+ `slide_from?`, `slide_to?`; **ignored for `.ppt`**) | `{ kind, slide_count }` / Markdown |
| `pptx_info` / `pptx_extract_markdown` | `path` / `bytes` (+ `slide_from?`, `slide_to?`) | `{ kind, slide_count }` / Markdown |
| `csv_info` / `csv_extract_markdown` | `path` / `bytes` (+ `row_from?`, `row_to?`) | `{ kind, row_count, column_count, header }` / Markdown table |

Ranges are 0-based, inclusive-from / exclusive-to, resolved and clamped by `domain::ranges::resolve_range`; `from > to` is a `Malformed` request. Params structs derive `serde::Deserialize` + `schemars::JsonSchema`. The removed tools (`classify_pdf`, `extract_pdf`, `extract_pdf_text`, `extract_document`) are **gone** — use `classify_document` + the matching `<format>_info` / `<format>_extract_markdown` tool.

#### Annotations and per-tool guidance (M04)

Every tool is **read-only**: it only reads/parses a document at the given `path`/`bytes` and never modifies its environment. Consequently each tool is declared with explicit per-tool **Annotations** (via the `rmcp` `#[tool(annotations(...))]` attribute) so clients can trust them without guessing:

- `readOnlyHint: true` — the tool never modifies its environment.
- `destructiveHint: false` — no destructive/write behavior (meaningful only when `readOnlyHint` is false).
- `idempotentHint: true` — repeating a call with the same arguments has no additional effect.
- `openWorldHint: false` — the tool's domain of interaction is closed (only the files/bytes passed in).

Each tool also carries a human-readable `title` and an extended per-tool `description` that doubles as its **per-tool guidance**: which format it handles, the `path` (within `ALLOW_ROOT_DIR`) vs base64 `bytes` input source, any applicable range params, Markdown-only output, and the read-only/safety caveat. The MCP spec has **no per-tool `instructions` field** — `instructions` is server-level only (carried by the `#[tool_handler(...)]`). Per-tool model guidance is therefore delivered through `description` (plus `title`/`annotations.title`), which the model sees as the authoritative usage note for each tool.

### 8.3 Error handling strategy

Two distinct channels per the `rmcp` model:

1. **Tool-level errors** — `Ok(CallToolResult::error(vec![...]))`. Use for *expected* business failures the caller should see as a message: unsupported/malformed/encrypted input, OCR unavailable, resource limit, wrong document kind for a tool. The caller receives a readable message and the tool "fails gracefully."
2. **Protocol errors** — `Err(McpError::invalid_params(...))`. Use only for *malformed tool invocations* (bad params, unknown fields, wrong types) — i.e. bugs on the client side.

**Rule of thumb:** bad *document data* → tool-level error; bad *request shape* → protocol error.

### 8.4 Dual-mode binary: MCP stdio server + CLI

The **same binary** serves both local MCP (stdio, primary) and a standalone CLI (costs nothing). Dispatch in `main.rs` parses argv: a recognized subcommand runs the CLI; otherwise it runs the MCP stdio server.

- `classify <path>` — print the classification (kind; for PDFs, `pdf_type` + pages needing OCR).
- `extract <path>` — print extracted Markdown (any supported format).
- `serve` — run the MCP stdio server (also the default when no recognized subcommand is given).

The CLI reuses the same application use cases and adapters; both modes share one binary and one set of ports.

## 9. OCR Routing Design

`pdf-inspector` and `anydoc` do **not** perform OCR and do **not** render PDF pages to images. The OCR engine (`tesseract-rs`) operates on a **bitmap image**, not a PDF. The OCR path therefore requires an explicit **PDF page → image renderer** step *before* the OCR provider.

The OCR engine is **vendored/compiled into the binary at build time** and the renderer is **pure Rust**, so the whole OCR+render stack statically links into the **single self-contained binary** with zero runtime system dependencies. Nothing is shelled out; no `tessdata` files exist on disk at runtime.

```
                 ┌────────────────────────────┐
   PDF ──▶ classify ──▶ PdfType + pages_needing_ocr (pdf-inspector, per page)
                 └────────────┬───────────────┘
                              │
              ┌───────────────┴───────────────┐
              ▼                               ▼
   text-based pages                     scanned/image pages
              │                               │
   PdfTextSource (per-page)          PdfPageRenderer (pure-Rust
   (pdf-inspector)                   rasterizer → PageBitmap)
              │                               │
              │                               ▼
               │                     OcrProvider (tesseract-rs)
               │                     (init cache_dir "eng+rus",
               │                      try_clone per worker,
               │                      spawn_blocking, semaphore
               │                      bounded by OCR_CONCURRENCY)
              │                               │
              └───────────────┬───────────────┘
                              ▼
                  Merge in page order (PdfUseCase)
                              ▼
                          Markdown
```

**Design decisions:**
- The `OcrProvider` **port** keeps OCR pluggable: `TesseractOcrProvider` (tesseract-rs) when `ocr-native` is on, `NoopOcrProvider` (returns `OcrUnavailable`) when off.
- The reference `TesseractOcrProvider` uses **`tesseract-rs`** with `embed-tessdata`: Tesseract+Leptonica compiled at build time and **`eng`+`rus` tessdata embedded**. It initializes by **materializing the embedded tessdata into a stable per-user cache dir and calling `api.init(cache_dir, "eng+rus")?`** (the `init_embedded("eng+rus")` form is broken — see §4.2.1) and consumes a **`PageBitmap`** (`ocr_page(bitmap: &PageBitmap)`).
- **PDF page → image rendering** is a separate concern performed *before* OCR, behind the `PdfPageRenderer` port: for each `pages_needing_ocr` page, render it to a **grayscale bitmap at ~300 DPI** and feed it to OCR. The renderer is **`pdf_render`** (pure Rust, no native deps).
- **Concurrency:** OCR is **real and bounded**. Scanned pages within a request are processed in parallel via a **semaphore sized to `OCR_CONCURRENCY`** (default CPU cores) + a `tokio::task::JoinSet` of `spawn_blocking` workers, each using a **`try_clone()`** of the single initialized API. The page count per request is **capped by `OCR_MAX_PAGES`** (default 300). This is not merely a claim — it is implemented in `PdfUseCase::ocr_pages_concurrently`.
- **Mixed-PDF merge:** **resolved** — because both `PdfTextSource` (per-page) and the OCR path produce per-page `ExtractedPage`s, a Mixed PDF merges text and OCR pages **interleaved in page order** (per-page interleaving is no longer a limitation).
- The renderer + OCR provider are both infrastructure-only; because both are injected and isolated behind ports, swapping implementations is a pure infrastructure change.
- If OCR is unavailable for a scanned doc, fail **explicitly** (`OcrUnavailable`), never silently return partial/empty content.

## 10. Security / Trust Model & Configuration

### 10.1 Trust model

- **Path access:** treat `path` inputs as untrusted (MCP clients may be driven by untrusted prompts). The server **canonicalizes** every path (resolving symlinks and expanding a leading `~`) and **restricts reads to `ALLOW_ROOT_DIR`** (default `~/Documents` on Unix, `/` fallback when no home; the container sets `ALLOW_ROOT_DIR=/data`). Path files are opened with **`O_NOFOLLOW`** and `fstat`'d on the same descriptor, closing the check/read TOCTOU. This is an **arbitrary-file-read-by-design** boundary: an untrusted client (or one steered by prompt injection) can read any file within `ALLOW_ROOT_DIR` that the process user can read. The MCP client is assumed trusted.
- **Size limits:** `MAX_INPUT_BYTES` (default 250 MiB) caps input size — enforced **before** base64 decode and before/while reading a path file. Exceeding → `ExtractionError::ResourceLimit` → tool-level error, never an OOM.
- **Bytes mode:** prefer content-based detection over trusting a caller-supplied extension (extension is only a fallback for contentless formats like CSV).
- **Encrypted files:** return `Encrypted` without attempting to bypass.
- **OCR is self-contained:** OCR runs via `tesseract-rs` (vendored, embedded tessdata) in the same process — **no** external binary, no shell-out.
- **Bounded OCR:** `OCR_MAX_PAGES` caps pages OCR'd per request; `OCR_CONCURRENCY` bounds OCR parallelism; `MAX_RETURN_CHARS` caps returned output for all extractors.
- **Decompression cap:** `MAX_DECOMPRESSED_BYTES` caps each decompressed archive member (e.g. a PPTX slide part), guarding against decompression bombs.
- **Panic isolation:** parsing/conversion is wrapped in `catch_unwind`, so a malformed document becomes a tool-level error, never a crash.
- No arbitrary command execution; no network egress from the core library.

### 10.2 Configuration (environment variables)

All settings have safe defaults, are unprefixed (no `MCP_`), and are read at call time (so tests can set them per-case). Defined in `src/config.rs`.

| Env var | Default | Purpose |
|---------|---------|---------|
| `MAX_INPUT_BYTES` | `250 * 1024 * 1024` (250 MiB) | Hard cap on input size (path file size, or decoded base64 `bytes`); enforced before read/decode. Exceeding → `ResourceLimit`. |
| `OCR_MAX_PAGES` | `300` | Per-request cap on the number of PDF pages OCR'd. Exceeding → `ResourceLimit`. |
| `OCR_CONCURRENCY` | CPU cores (`available_parallelism`) | Max PDF pages OCR'd in parallel per request (semaphore). |
| `MAX_RETURN_CHARS` | `50_000_000` | Safety-net cap on the returned output size for all extractors (doc/docx/rtf `word_range`, ppt/pptx, csv, xls/xlsx). Exceeding → `ResourceLimit`. |
| `MAX_DECOMPRESSED_BYTES` | `256 * 1024 * 1024` (256 MiB) | Cap on the decompressed size of a single archive member (e.g. a PPTX slide part) — guards against decompression bombs. Exceeding → `ResourceLimit`. |
| `ALLOW_ROOT_DIR` | `~/Documents` (Unix; `/` fallback if no home) / system volume root (Windows) | Every `path` is canonicalized (leading `~` expanded) and must be within this root; paths outside are rejected. The container sets `ALLOW_ROOT_DIR=/data`. |

## 11. Testing & Coverage Strategy (default ≥ 95%; `ocr-native` ≥ 85%)

The coverage goal is achieved by testing each layer at the right level. The suite runs in **both configurations**: the default (pure-Rust, `NoopOcrProvider`) build targets **≥ 95%** line coverage, and the full `ocr-native` build (real `TesseractOcrProvider`) targets **≥ 85%** (the heavier OCR path has a smaller dedicated surface).

### 11.1 Unit tests (domain + application)

- **Domain:** pure entities, value objects, error types, `resolve_range` — straightforward unit tests.
- **Application:** use cases tested against **fake/stub ports** (in-memory `FakeExtractor`, `FakeOcrProvider`, `FakePageRenderer`, `FakePdfTextSource`, fake readers) — no real OCR/render, no filesystem. This exercises classification, routing, OCR-cap/concurrency, merge order, OCR-unavailable handling, and error mapping.
- Keep the heavy logic in **pure, fake-testable functions**; keep the thin adapter glue small so most of the code path is covered by unit tests.
- Use `#[cfg(test)]` modules inside each file plus `#[tokio::test]` for async use cases (e.g. `PdfUseCase`).

### 11.2 Integration tests (infrastructure adapters)

- Adapters tested against the committed fixtures in `tests/fixtures/` (`simple_text.docx`, `simple_text.pdf`, `simple_text.xlsx`) plus generated in-memory files (`tests/format_adapters.rs` builds PPTX zips, temp CSVs).
- Because OCR uses `tesseract-rs` with **embedded tessdata** (no runtime system dependencies), the real OCR integration tests **run normally on any machine** — they are **not** skipped for lack of a system Tesseract. (They exercise the real OCR path, so the build must have embedded `eng`+`rus` tessdata.)
- **`ocr-native` integration test:** `tests/ocr_native.rs` (gated `#![cfg(feature="ocr-native")]`) runs the real `TesseractOcrProvider` + `PdfRenderer` on a rendered page (a **synthetic scanned page** with no text layer, so it genuinely requires OCR) and asserts non-empty Cyrillic-capable OCR.
- Note: the repo currently has **no** dedicated scanned/mixed/malformed/encrypted PDF fixtures committed — those paths are covered by unit tests and the `ocr-native` synthetic test. Adding such fixtures is future work.

### 11.3 Interface tests

- MCP tools tested by invoking the server handler with constructed params, asserting `CallToolResult` shape and error classification (including cross-kind rejections — e.g. `docx_info` on a PDF returns a tool-level `unsupported`). Avoid requiring a live MCP client.

### 11.4 Coverage measurement

```bash
cargo install cargo-llvm-cov
cargo llvm-cov --html
cargo llvm-cov --fail-under-lines 95          # default build gate
cargo llvm-cov --features ocr-native --fail-under-lines 85   # ocr-native gate
```

### 11.5 Testing tools

- `cargo llvm-cov` — coverage (the CI gate for the 95% line target on the default build; 85% on `ocr-native`).
- `#[tokio::test]` — async use-case tests.
- Fake/stub ports (in-memory implementations of the domain ports) — application-layer isolation.

## 12. CI

A minimal CI pipeline runs on every PR/merge to the default branch. The **full OCR binary is built in a Docker multi-stage build** (builder pre-provisions `rus.traineddata` + sets `TESSERACT_EMBED_LANGUAGES=eng,rus` + `--features ocr-native`); the runtime and the coverage gate need **no system Tesseract**. The builder needs a C++ compiler + CMake + internet (to compile/download vendored Tesseract/Leptonica + tessdata) — CI does **not** install tesseract at runtime.

1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test`
4. `cargo llvm-cov --fail-under-lines 95`   (default build)

The **line-coverage gate is ≥ 95% on the default (pure-Rust) build** and **≥ 85% on the Docker/CI `ocr-native` build**, both on a bare runner (no runtime OCR packages). Measured on the default build: **~97%** (comfortably ≥ 95%); the `ocr-native` build sits **~91%** (≥ 85%). (The vendored C++ build is slow on first compile — cache the compiled libraries / Docker builder layers between runs.)

Artifacts: coverage report; fail the build if the default-build 95% / `ocr-native` 85% line gate is not met.

## 13. Dependencies

```toml
[dependencies]
rmcp = { version = "3.1.2", features = ["server", "transport-io"] }
anydoc = "0.1.8"                 # whole-document fallback / word-formats / legacy .ppt
pdf-render = "1.0.0-beta.17"     # pure-Rust PDF rasterizer
pdf-inspector = "0.1.8"          # per-page PDF text + classification (PDF path)
csv = "1.4"                      # streaming CSV
calamine = "0.36"                # xls/xlsx
zip = { version = "8.6", default-features = false }   # PPTX OOXML
quick-xml = "0.41"               # slide XML parsing
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
schemars = "1"
base64 = "0.22"                  # decode optional `bytes` input
# optional, gated behind the `ocr-native` feature (default OFF):
tesseract-rs = { version = "0.4.0", features = ["embed-tessdata"], optional = true }

[dev-dependencies]
tokio-test = "0.4"
```

**Feature gate:** `tesseract-rs` is an **optional dependency** enabled by the `ocr-native` feature (default OFF). The default build is fully pure-Rust and needs **no C++ toolchain**. The full OCR binary is built with `--features ocr-native` in Docker/CI.

**No runtime system prerequisites.** Runtime has zero OS packages: no `libtesseract`, no `libleptonica`, no `tessdata` files, no `TESSDATA_PREFIX`. **Build-time requirements for `ocr-native`** (NOT runtime, NOT default build): a C++ compiler (`gcc`/`clang`), **CMake**, and **internet access** (to download Tesseract/Leptonica sources + training data).

**Build env var (required for `ocr-native`):** the build must set `TESSERACT_EMBED_LANGUAGES=eng,rus` (default is `eng,tur` — Russian is **not** included by default) and **pre-provision `rus.traineddata`** into the tesseract-rs tessdata cache dir before building (the crate only auto-downloads `eng`/`tur`). Use the provided **Docker multi-stage build** / `make build` so this is not forgotten. See §4.2.1/§4.2.3.

## 14. Roadmap

| Round | Scope | Notes |
|-------|-------|-------|
| **R1 (done)** | PDF classification + per-page text extraction + OCR routing (pure-Rust PDF→image renderer + `tesseract-rs` OCR with embedded eng+rus tessdata); single self-contained binary; 95%/85% coverage (default/ocr-native) | `pdf-inspector` + `rmcp` + `tesseract-rs` (embed-tessdata) + pure-Rust renderer |
| **R2 (done)** | Per-format tool surface — `_info`/`_extract_markdown` for pdf/doc/docx/rtf/xls/xlsx/ppt/pptx/csv with dedicated adapters (`pdf-inspector`, `calamine`, `csv`, `zip`+`quick-xml`, `anydoc`); ranges per format; real bounded page-parallel OCR; `.xls`/`.xlsx` distinguished by magic bytes; mixed-PDF per-page interleaving resolved | Per-format use cases + adapters; `classify_document` retained as the single generic classifier |
| **R3 (candidate)** | OCR quality, further backpressure/large-file streaming, optional HTTP transport, dedicated ODF/EPUB tools, scanned/mixed fixture coverage | Depends on feedback |

## 15. Open Questions

- **Renderer:** **DECIDED** — `pdf_render` is the chosen PDF page → image renderer (see §4.3); `rasterrocket` is rejected (native FreeType/OpenJPEG + broken crates.io), and `mupdf`/`pdfium-render` remain dropped. No longer open.
- **`rus` embedding + Cyrillic accuracy:** **VERIFIED working** — the Docker build (`--features ocr-native`) embeds `eng`+`rus`, the cache-dir `init` workaround (with size-check fast path) works end-to-end, and a real scanned Russian receipt PDF produced correct Cyrillic OCR (~1356 chars). The `init_embedded("eng+rus")` gotcha remains documented (§4.2.1). OCR quality still depends on scan quality; the **`tessdata_best` vs `fast`** size/accuracy tradeoff remains (see below).
- **Binary size / tessdata variant:** **DECIDED — ship `tessdata_best`** for OCR accuracy. The Dockerfile provisions `tessdata_best` (eng+rus ≈ 30.7 MB, ~22 MB over `fast`); the resulting single binary is ~50–72 MB, which is acceptable. `tessdata_fast` remains the smaller fallback (≈ 8 MB eng+rus) if binary size matters later (see §4.2.1).
- **OCR concurrency:** **IMPLEMENTED** — real bounded page-parallel OCR (semaphore sized to `OCR_CONCURRENCY` + `JoinSet` of `spawn_blocking` workers using `try_clone()`, capped by `OCR_MAX_PAGES`) in `PdfUseCase::ocr_pages_concurrently` (§7.2/§9). Minor remaining item: confirm clone-pool sizing (cores) vs the per-request-init fallback for production tuning.
- **OCR image preparation:** exact page→image parameters (resolution ~300 DPI, grayscale via `pixmap.data_as_u8_slice()` → luma) — validated by the renderer spike.
- **CLI vs MCP-only:** **DECIDED** — the binary ships **both**: MCP stdio server (default) and a standalone CLI (`classify`/`extract`/`serve`), dispatched in `main.rs` (see §8.4). No longer open.
- **Per-format tool surface:** **DECIDED** — the per-format `_info`/`_extract_markdown` tools ship alongside the single generic `classify_document`; the old `classify_pdf`/`extract_pdf`/`extract_pdf_text`/`extract_document` tools are removed (§8.2). No longer open.
- **`.xls`/`.xlsx` labeling:** **RESOLVED** — `.xls` vs `.xlsx` are now distinguished by **magic bytes** (`.xlsx` = ZIP `PK\x03\x04`; `.xls`/`.xlsb` = OLE `D0 CF 11 E0`) in `AnyDocExtractor::excel_kind`. The old "`.xls` is labeled `Xlsx`" caveat no longer holds.
- **Mixed-PDF per-page interleaving:** **RESOLVED** — PDF text extraction is now per-page (via `pdf-inspector`), so a Mixed PDF interleaves text and OCR pages in page order (§7.2/§9). The old "per-page interleaving not available" limitation is gone.
- **ODF / EPUB tools:** whether to add dedicated `_info`/`_extract_markdown` tools for `odt`/`ods`/`odp`/`epub` (currently only classified + CLI whole-document fallback via `anydoc`).
- **Fixtures:** whether to commit dedicated scanned/mixed/malformed/encrypted PDF fixtures (currently covered by unit tests + the `ocr-native` synthetic test).
- **Remote OCR:** whether a network OCR provider is required or if the pure-Rust engine (and stub) is sufficient.
- **Crate layout:** single-crate modules (default here) vs Cargo workspace — finalize when the codebase grows.
