# cargo-chef is a tool that correctly handles Rust dependency caching in Docker.
# It works in three stages:
#   planner  — analyses the workspace and produces a dependency "recipe"
#   builder  — cooks (compiles) deps from the recipe, then builds the real binary
#   runtime  — minimal image with only the compiled binary

# Pre-built image: cargo-chef installed on top of the official Rust image
FROM lukemathwalker/cargo-chef:latest-rust-1-slim-bookworm AS chef

WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*


# ── Stage 1: planner ──────────────────────────────────────────────────────────
# Reads the workspace (Cargo.toml files + Cargo.lock) and emits recipe.json —
# a compact description of all dependencies. No source code is compiled here.
FROM chef AS planner

COPY . .
RUN cargo chef prepare --recipe-path recipe.json


# ── Stage 2: builder ──────────────────────────────────────────────────────────
# "Cook" step: builds all dependencies from recipe.json only.
# Docker caches this layer as long as recipe.json (i.e. Cargo.toml / Cargo.lock)
# doesn't change — so editing your own source doesn't re-download or re-compile deps.
#
# "Build" step: copies the real source and compiles only the application code.
FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release -p yan-server


# ── Stage 3: runtime ──────────────────────────────────────────────────────────
# Start fresh from a minimal Debian image — no Rust toolchain, no build tools.
# The final image contains only the binary and its runtime system dependencies.
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.source="https://github.com/shamblashini/yan"
LABEL org.opencontainers.image.description="Yan sync server"
LABEL org.opencontainers.image.licenses="MIT"

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Run the server as a non-root user
RUN useradd --system --no-create-home --shell /usr/sbin/nologin yan

COPY --from=builder /build/target/release/yan-server /usr/local/bin/yan-server

USER yan
EXPOSE 3000

HEALTHCHECK --interval=15s --timeout=5s --start-period=15s --retries=3 \
    CMD wget -qO /dev/null http://localhost:3000/health || exit 1

CMD ["yan-server"]
