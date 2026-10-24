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

# --- Mutation testing (cargo-mutants) hard gate (criterion C02/N15) ---
# `make mutation` is the language-neutral interface the CI `mutation` job calls.
#   * MUTANTS_VERSION   — pinned cargo-mutants version. `make mutation` installs
#     this exact version (if missing or a different one is present) so the CI
#     gate is reproducible.
#   * MUTATION_MIN_SCORE— minimum mutation score (caught / viable mutants) below
#     which the gate fails. This is a hard gate, not a suggestion.
#   * MUTANTS_TIMEOUT   — per-mutant test timeout in seconds (guards against
#     mutation-induced hangs / infinite loops; lowered to 60 so a hanging
#     mutant costs 60s, not 300s, keeping the run inside the CI job timeout).
#   * MUTANTS_JOBS      — test-worker parallelism, capped to 8 to bound CI load.
#   * MUTANTS_FILES     — optional space-separated list of source paths passed
#     to `cargo mutants --file <...>` to bound the mutation scope (empty = all
#     files). Lets CI run a bounded, high-value subset so the whole job fits in
#     the `mutation` job's `timeout-minutes`.
MUTANTS_VERSION    := 27.1.0
MUTATION_MIN_SCORE := 80
MUTANTS_TIMEOUT    := 60
MUTANTS_JOBS       := $(shell getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)
# `?=` (not `:=`) so a MUTANTS_FILES set in the environment (the CI `mutation`
# job) is honored instead of being clobbered by this empty default. Without
# this, cargo-mutants runs on ALL files, which both blows the CI job timeout
# and fails the gate on untestable-in-default-build files (tesseract_ocr.rs,
# interface/*).
MUTANTS_FILES      ?=

.PHONY: build build-local prepare-tessdata test coverage fmt lint check-ocr check mutation

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

## Line coverage (≥ 95% target on the default build) via cargo-llvm-cov.
coverage:
	$(CARGO) llvm-cov --html
	$(CARGO) llvm-cov --fail-under-lines 95

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

## Mutation testing as a HARD gate (criterion C02/N15).
## Runs cargo-mutants on the DEFAULT (pure-Rust) build only — never ocr-native
## (that needs a C++ toolchain + CMake + rus.traineddata, exercised separately by
## `build`/`check-ocr` and the `ocr-native` CI job). tests/ocr_native.rs is
## `#[cfg(feature = "ocr-native")]`-gated and therefore compiles out of (and is
## not exercised by) the default build.
##
## NOTE: do NOT combine `--in-place` with `-j` — cargo-mutants declares them as
## mutually conflicting (usage error, exit 1). We therefore run the default
## (scratch-copy) mode with `-j` parallelism for CI speed.
##
## Fails (non-zero exit) when the mutation score is below MUTATION_MIN_SCORE or
## when the run itself is broken (baseline failure / no viable mutants / no
## report), so it can serve as a hard gate in CI.
mutation:
	@set -e; \
	set -f; \
	MUTANTS_VERSION="$(MUTANTS_VERSION)"; \
	MIN_SCORE="$(MUTATION_MIN_SCORE)"; \
	JOBS="$(MUTANTS_JOBS)"; \
	FILES="$(MUTANTS_FILES)"; \
	if [ "$$JOBS" -gt 8 ]; then JOBS=8; fi; \
	if command -v cargo-mutants >/dev/null 2>&1; then \
		installed="$$(cargo mutants --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n1)"; \
		if [ "$$installed" != "$$MUTANTS_VERSION" ]; then \
			echo "==> cargo-mutants $$installed != pinned $$MUTANTS_VERSION; reinstalling"; \
			cargo install cargo-mutants --version "$$MUTANTS_VERSION"; \
		else \
			echo "==> using cargo-mutants $$MUTANTS_VERSION"; \
		fi; \
	else \
		echo "==> installing cargo-mutants $$MUTANTS_VERSION (pinned)"; \
		cargo install cargo-mutants --version "$$MUTANTS_VERSION"; \
	fi; \
	if [ -n "$$FILES" ]; then \
		SCOPE_ARG=""; \
		for f in $$FILES; do SCOPE_ARG="$$SCOPE_ARG --file $$f"; done; \
		echo "==> running cargo mutants on bounded scope (jobs=$$JOBS, timeout=$(MUTANTS_TIMEOUT)s, files: $$FILES)"; \
	else \
		SCOPE_ARG=""; \
		echo "==> running cargo mutants on ALL files (jobs=$$JOBS, timeout=$(MUTANTS_TIMEOUT)s)"; \
	fi; \
	rm -rf mutants.out; \
	set +e; \
	$(CARGO) mutants -j "$$JOBS" --timeout "$(MUTANTS_TIMEOUT)" -v $$SCOPE_ARG; \
	MUTANTS_RC=$$?; \
	set +f; \
	set -e; \
	echo "==> cargo-mutants exit code: $$MUTANTS_RC"; \
	if [ "$$MUTANTS_RC" -ne 0 ] && [ "$$MUTANTS_RC" -ne 2 ] && [ "$$MUTANTS_RC" -ne 3 ]; then \
		echo "FATAL: cargo-mutants exited $$MUTANTS_RC (usage / baseline failure / internal error); the mutation gate cannot be evaluated."; \
		exit "$$MUTANTS_RC"; \
	fi; \
	python3 -c 'import sys,json,os\nm=float(sys.argv[1])\np="mutants.out/outcomes.json"\nif not os.path.exists(p):\n    print("ERROR: %s missing; the mutation run produced no report" % p)\n    sys.exit(1)\nd=json.load(open(p))\nt=int(d.get("total_mutants",0)); c=int(d.get("caught",0)); miss=int(d.get("missed",0)); to=int(d.get("timeout",0)); u=int(d.get("unviable",0))\nv=t-u\nscore=(100.0*c/v) if v>0 else 0.0\nprint("mutation report: total=%d caught=%d missed=%d timeout=%d unviable=%d" % (t,c,miss,to,u))\nprint("mutation score = %.1f%% (hard-gate minimum %.0f%%)" % (score,m))\nif t==0:\n    print("ERROR: no mutants were generated/tested; the gate would be a fiction")\n    sys.exit(1)\nif v<=0:\n    print("ERROR: no viable mutants (all unviable); the gate would be a fiction")\n    sys.exit(1)\nif score<m:\n    print("FAIL: mutation score %.1f%% is below the %.0f%% hard gate; inspect mutants.out/missed.txt and close the gap (add tests, or a justified #[mutants::skip]/exclude for unreachable stubs)." % (score,m))\n    sys.exit(1)\nprint("PASS: mutation gate satisfied")' "$$MIN_SCORE"
