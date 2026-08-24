# Security Policy

## Reporting a vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Instead, report them privately via **GitHub Security Advisories**:

1. Go to **Security → Advisories → New advisory** on this repository (or use
   the "Report a vulnerability" button on the repo page).
2. Include a clear description, the affected version/commit, and a minimal
   reproduction.

We aim to acknowledge reports within a few business days and to coordinate a
fix and a coordinated disclosure. Please give us reasonable time to respond
before disclosing publicly.

## Scope

This project is an **MCP (Model Context Protocol) server** that extracts text /
Markdown from local documents — PDF (with OCR), MS Office, ODF, RTF, EPUB, CSV.

The following are in scope for security review:

- Unauthorized file access beyond the intended read scope.
- Resource exhaustion (memory / CPU) via crafted input.
- Crashes / denial-of-service caused by malformed documents.
- Panic or unexpected process abort from untrusted input.

## Trust model

> ⚠️ **Arbitrary file read by design.** The tools read **local file paths**
> (`path`). This server is intended to run on a machine the MCP client trusts.
> An untrusted MCP client — or one steered by untrusted prompt injection — can
> read any file the process user can read. **Treat the MCP client as trusted**
> and do not expose this server to untrusted clients / the network without
> additional access control in front of it.

The following controls bound that model:

- **`ALLOW_ROOT_DIR`** (default `/`) — every `path` is **canonicalized** (symlinks
  resolved) and must fall within this root; paths outside it are rejected. Set
  it to a specific directory to narrow the read scope.
- **`MAX_INPUT_BYTES`** (default 250 MiB) — caps input size (file size or
  decoded base64 `bytes`) before read/decode; exceeding it returns a
  `ResourceLimit` tool-level error rather than exhausting memory.
- **Bounded OCR** — `OCR_MAX_PAGES` (default 300) caps pages OCR'd per request
  and `OCR_CONCURRENCY` (default CPU cores) bounds OCR parallelism; a request
  exceeding the page cap returns a `ResourceLimit` error.
- **`MAX_RETURN_CHARS`** (default 50M) — safety-net cap on doc/docx/rtf
  `word_range` output size.
- **No shell-out / no external execution** — OCR and rendering run in-process
  with embedded data; nothing is shelled out.
- **Panic isolation** — document parsing/conversion is wrapped in
  `catch_unwind`, so a malformed document becomes a tool-level error, never a
  process crash.

## Supply-chain security

CI runs **`cargo audit`** (RustSec advisory database for known
vulnerabilities) and **`cargo deny check`** (license + advisory + ban policy)
on every push/PR. The full `ocr-native` build compiles vendored Tesseract and
Leptonica from source and embeds `eng`+`rus` tessdata into a single
self-contained binary; nothing is fetched or executed at runtime.

## Supported versions

Only the latest release on the default branch is actively supported for
security fixes. Backports to older versions are generally not provided.
