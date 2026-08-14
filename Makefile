# mcp-documents — development / build wrapper.
#
# Two build modes:
#   * `make build-local` — default features (pure Rust, no C++/CMake needed).
#   * `make build`       — full self-contained binary WITH OCR (`ocr-native`).
#       Requires a C++ compiler + CMake + internet, and pre-provisioned
#       `rus.traineddata` (see SPEC §4.3). Best run via Docker (see Dockerfile).

# Rust/LLVM tools
CARGO    := cargo
RUSTFMT  := cargo fmt
CLIPPY   := cargo clippy

# Full (OCR-enabled) build env. The default tessdata languages are eng,tur —
# we require Russian, so embed eng+rus. `rus.traineddata` must be present in the
# tesseract-rs tessdata cache dir BEFORE this build (pre-provision it or build
# via Docker). See SPEC §4.3.
EMBED_LANGS := eng,rus

.PHONY: build build-local test coverage fmt lint check-ocr check

## Full self-contained binary with OCR (needs C++/CMake + rus.traineddata).
build:
	TESSERACT_EMBED_LANGUAGES=$(EMBED_LANGS) $(CARGO) build --release --features ocr-native

## Local build with default features (pure Rust, no OCR).
build-local:
	$(CARGO) build

## Run all tests (default features).
test:
	$(CARGO) test

## Line coverage (≥ 85% target) via cargo-llvm-cov.
coverage:
	$(CARGO) llvm-cov --html
	$(CARGO) llvm-cov --fail-under-lines 85

## Format source.
fmt:
	$(RUSTFMT)

## Lint with warnings denied.
lint:
	$(CLIPPY) --all-targets -- -D warnings

## Compile-check the OCR feature (still needs C++/CMake + rus.traineddata).
check-ocr:
	TESSERACT_EMBED_LANGUAGES=$(EMBED_LANGS) $(CARGO) check --features ocr-native

## Format check + lint + test (CI gate equivalents).
check: fmt lint test
