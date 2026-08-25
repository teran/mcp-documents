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
# we require Russian, so embed eng+rus. `prepare-tessdata` guarantees both
# eng.traineddata and rus.traineddata exist in the tesseract-rs cache dir
# before building. See SPEC §4.3.
EMBED_LANGS := eng,rus

# tesseract-rs tessdata cache dir (per-OS), and the source mirror for the
# training data. We always need English + Russian.
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
TESSDATA_DIR := $(HOME)/Library/Application Support/tesseract-rs/tessdata
else
TESSDATA_DIR := $(HOME)/.tesseract-rs/tessdata
endif
TESSDATA_MIRROR := https://cdn.jsdelivr.net/gh/tesseract-ocr/tessdata_best@main
TESSDATA_LANGS  := eng rus

.PHONY: build build-local prepare-tessdata test coverage fmt lint check-ocr check

## Ensure eng.traineddata + rus.traineddata exist in the tesseract-rs cache dir
## (download them from a mirror if missing). Required before any `ocr-native`
## build, otherwise `rus` silently fails to embed.
prepare-tessdata:
	mkdir -p "$(TESSDATA_DIR)"
	@for lang in $(TESSDATA_LANGS); do \
		f="$(TESSDATA_DIR)/$$lang.traineddata"; \
		if [ ! -f "$$f" ]; then \
			echo "==> downloading $$lang.traineddata -> $$f"; \
			curl -fL -o "$$f" "$(TESSDATA_MIRROR)/$$lang.traineddata"; \
		else \
			echo "==> $$lang.traineddata already present"; \
		fi; \
	done
	@# Supply-chain guard: verify rus against the pinned SHA-256 (matches
	@# RUS_TESSDATA_SHA256 in src/infrastructure/tesseract_ocr.rs).
	@echo "b617eb6830ffabaaa795dd87ea7fd251adfe9cf0efe05eb9a2e8128b7728d6b6  $(TESSDATA_DIR)/rus.traineddata" | sha256sum -c -

## Full self-contained binary with OCR (needs C++/CMake + eng+rus tessdata).
build: prepare-tessdata
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
