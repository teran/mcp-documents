# syntax=docker/dockerfile:1
#
# Multi-stage build for the single self-contained binary with OCR.
#
# Builder: compiles vendored Tesseract+Leptonica (needs C++ + CMake + internet),
# pre-provisions `rus.traineddata`, and embeds eng+rus tessdata.
# Runtime: ships ONLY the single static binary — zero system deps.

########## Builder ##########
# Pinned by digest for supply-chain reproducibility.
FROM rust:1.97.1-trixie@sha256:b1b3c9c0d921d7fa0a6d1f9ec7e4eab87f8c8ec97644c3d791450f131dec813f AS builder

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
# flaky GitHub raw, then verify its SHA-256 (matches RUS_TESSDATA_SHA256 in
# src/infrastructure/tesseract_ocr.rs) so a tampered/unchanged model fails the
# build instead of being embedded. We use `tessdata_best` (accurate LSTM models)
# — it is larger (rus ≈ 15 MB vs ≈ 4 MB for `fast`), adding ~22 MB to the binary
# over `fast`. Switch to tessdata_fast only together with the pinned digest.
RUN mkdir -p ~/.tesseract-rs/tessdata \
    && curl -fL -o ~/.tesseract-rs/tessdata/rus.traineddata \
         "https://cdn.jsdelivr.net/gh/tesseract-ocr/tessdata_best@main/rus.traineddata" \
    && echo "b617eb6830ffabaaa795dd87ea7fd251adfe9cf0efe05eb9a2e8128b7728d6b6  ~/.tesseract-rs/tessdata/rus.traineddata" | sha256sum -c -

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

# Prepare an empty runtime tessdata cache dir for the distroless stage.
# Distroless images have NO shell, so `RUN` cannot create dirs there — we
# create the cache dir here (builder) and COPY it into the runtime stage below.
# NOTE: Docker COPY does not preserve a source directory's own ownership, so we
# must use `COPY --chown=65534:65534` on the runtime side to make `/cache`
# owned by `nobody` (owner rwx) for the embedded-tessdata materialization.
RUN mkdir -p /app/rootfs/cache

########## Runtime ##########
# ABI decision: the binary is dynamically linked against glibc + libstdc++
# (Tesseract/Leptonica are statically linked, but glibc/libstdc++ are dynamic).
# Built on Debian trixie, it requires GLIBC_2.38 / GLIBCXX_3.4.31. We therefore
# use `gcr.io/distroless/cc-debian13` — Debian 13 (trixie), glibc 2.41 — which
# MATCHES the builder's ABI and ships glibc + libstdc++ + libgcc.
# DO NOT switch to `cc-debian12` (bookworm, glibc 2.36): it cannot load a
# trixie-built binary. The `cc` variant (not `base`/`static`) is required
# because it bundles libstdc++ alongside glibc. Pinned by digest for
# supply-chain reproducibility.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:a77defd6fedbb3392b175ba8ea3d1c22be963c1597c248c3ba987ddd80bfb512 AS runtime

# The binary is self-contained with respect to OCR: embedded eng+rus tessdata,
# pure-Rust renderer, static OCR libs. Only the base glibc/libstdc++ are needed.
WORKDIR /app
COPY --from=builder /app/bin/mcp-documents /usr/local/bin/mcp-documents

# Non-root hardening: run as `nobody` (UID 65534). Because distroless has no
# shell, the writable tessdata cache dir is prepared in the builder and copied
# in here with `--chown=65534:65534` (Docker does not preserve a source dir's
# ownership, so this flag is required). We point XDG_CACHE_HOME at it so the
# provider's primary cache path resolves to /cache/mcp-documents/tessdata.
# (The provider falls back to $HOME/.cache and then /tmp; /tmp is already
# present in this image and world-writable (1777), so it also works for nobody.)
COPY --from=builder --chown=65534:65534 /app/rootfs/cache /cache
ENV XDG_CACHE_HOME=/cache

# Sandbox the read scope: this container is a service, so `~/Documents` (the
# default when ALLOW_ROOT_DIR is unset) does not exist here. Restrict reads to
# /data — mount the directory of documents you want the server to be able to
# read at /data (see SECURITY.md).
ENV ALLOW_ROOT_DIR=/data

# The `:nonroot` base tag defaults to UID 65532; we explicitly target nobody
# 65534 to match the writable /cache above. Using the numeric UID is
# unambiguous (no getpwnam needed; /etc/passwd also defines nobody:65534).
USER 65534

# Entrypoint: runs as an MCP stdio server by default; also usable as a CLI.
# Distroless has no shell, so ENTRYPOINT/CMD must stay in exec (vector) form.
ENTRYPOINT ["/usr/local/bin/mcp-documents"]

# Read-only rootfs guidance: distroless already ships no shell/package manager.
# The only writable paths are /cache (tessdata) and /tmp, both regenerable. For
# a fully read-only rootfs run with `--read-only --tmpfs /tmp` (or mount a
# volume / emptyDir on /cache); the tessdata cache is re-materialized from the
# embedded bytes on each container start, so it need not persist.
