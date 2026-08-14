# syntax=docker/dockerfile:1
#
# Multi-stage build for the single self-contained binary with OCR.
#
# Builder: compiles vendored Tesseract+Leptonica (needs C++ + CMake + internet),
# pre-provisions `rus.traineddata`, and embeds eng+rus tessdata.
# Runtime: ships ONLY the single static binary — zero system deps.

########## Builder ##########
FROM rust:1.97.1-trixie AS builder

# Build-time deps: C++ toolchain + CMake (to compile vendored Tesseract/Leptonica)
# + curl (to fetch rus.traineddata from a mirror). None needed at runtime.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake curl pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifests first to leverage layer caching for dependency resolution.
COPY Cargo.toml Cargo.lock* ./

# Pre-provision rus.traineddata into the tesseract-rs tessdata cache dir BEFORE
# building. The crate only auto-downloads eng/tur, so rus must be present or
# embedding silently fails. Fetch from a pinned mirror (jsdelivr) to avoid
# flaky GitHub raw. `tessdata_fast` is much smaller than the crate's default
# `best` variant; adjust the URL if you prefer `best`.
RUN mkdir -p ~/.tesseract-rs/tessdata \
    && curl -fL -o ~/.tesseract-rs/tessdata/rus.traineddata \
         "https://cdn.jsdelivr.net/gh/tesseract-ocr/tessdata_fast@main/rus.traineddata"

# Embed English + Russian OCR (default is eng,tur — Russian is NOT included).
ENV TESSERACT_EMBED_LANGUAGES=eng,rus

# Copy the source and build the release binary with OCR enabled. BuildKit
# cache mounts keep the cargo registry + compiled deps across rebuilds so
# source-only changes recompile just the crate (not the whole tree).
COPY src ./src
# Note: `/app/target` is a BuildKit cache mount (ephemeral, not part of the
# image). We copy the finished binary to /app/bin/mcp-documents (in the real
# layer) so the runtime stage can COPY it out.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --features ocr-native \
    && mkdir -p /app/bin \
    && cp /app/target/release/mcp-documents /app/bin/mcp-documents

########## Runtime ##########
# Must match the builder's libc/libstdc++ ABI: the binary is dynamically linked
# against glibc + libstdc++ (Tesseract/Leptonica are statically linked, but
# glibc/libstdc++ are dynamic), so the runtime base must be the SAME Debian
# release as the builder (trixie), not bookworm (whose glibc 2.36 / older
# libstdc++ cannot run a trixie-built binary). No runtime packages are
# installed beyond the base image; the image ships only the single binary.
FROM debian:trixie-slim AS runtime

# The binary is self-contained with respect to OCR: embedded eng+rus tessdata,
# pure-Rust renderer, static OCR libs. Only the base glibc/libstdc++ are needed.
WORKDIR /app
COPY --from=builder /app/bin/mcp-documents /usr/local/bin/mcp-documents

# Entrypoint: runs as an MCP stdio server by default; also usable as a CLI.
ENTRYPOINT ["/usr/local/bin/mcp-documents"]
