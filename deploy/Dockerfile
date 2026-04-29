# syntax=docker/dockerfile:1.7
# Multi-stage build for mcp-oxide. Produces a distroless, non-root image
# containing only the static gateway binary.

ARG RUST_VERSION=1.88

# -----------------------------------------------------------------------------
# Stage 1: cargo-chef recipe (cache workspace dependency graph).
# -----------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS chef
WORKDIR /src
RUN cargo install cargo-chef --locked --version 0.1.68
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# -----------------------------------------------------------------------------
# Stage 2: build cached dependencies.
# -----------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /src
RUN cargo install cargo-chef --locked --version 0.1.68
COPY --from=chef /src/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build the actual workspace.
COPY . .
RUN cargo build --release --bin mcp-oxide --locked

# -----------------------------------------------------------------------------
# Stage 3: runtime — distroless nonroot.
# -----------------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

LABEL org.opencontainers.image.title="mcp-oxide"
LABEL org.opencontainers.image.description="MCP Gateway (Rust, cloud-agnostic, pluggable)"
LABEL org.opencontainers.image.source="https://github.com/anomalyco/mcp-oxide"
LABEL org.opencontainers.image.licenses="Apache-2.0 OR MIT"

USER nonroot:nonroot
WORKDIR /app

COPY --from=builder --chown=nonroot:nonroot /src/target/release/mcp-oxide /usr/local/bin/mcp-oxide

EXPOSE 8080 9090

ENTRYPOINT ["/usr/local/bin/mcp-oxide"]
