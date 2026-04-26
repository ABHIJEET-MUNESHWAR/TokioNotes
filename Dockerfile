# syntax=docker/dockerfile:1.7
# Multi-stage Dockerfile for the all-in-one TokioNotes gateway.
#
# Hardening notes:
#  * Pin a current stable Rust image. The image already ships a complete
#    toolchain — `RUSTUP_TOOLCHAIN=1.89.0` below pins us to *that* toolchain
#    so the workspace's `rust-toolchain.toml` (channel = "stable") does NOT
#    trigger a fresh rustup download inside the container — a common cause
#    of opaque `exit code: 1` failures on slow / restricted networks.
#    NOTE: several transitive deps (async-graphql 7.2, actix-web 4.13,
#    time 0.3.47, darling 0.23) require rustc >= 1.89, so the base image
#    must stay on 1.89 or newer.
#  * Use the **sparse** crates.io protocol — much smaller and resumable than
#    the legacy git index; avoids most of the "Operation too slow" stalls.
#  * Bump cargo's network timeout & retry counts so a slow build network
#    doesn't fail the whole compile.
#  * Use BuildKit cache mounts on the cargo registry + target dir so an
#    incremental rebuild is seconds, and the first rebuild after a network
#    flake doesn't have to re-download every crate.
#  * Force codegen-units=16 / LTO off so peak link-time RAM stays under
#    ~2.5 GB — Docker Desktop's default 2 GB memory limit otherwise OOM-kills
#    the linker silently and surfaces only as `exit code: 1`.
FROM rust:1.89-slim-bookworm AS builder
WORKDIR /app

# Force the image's preinstalled toolchain — overrides rust-toolchain.toml's
# `channel = "stable"` which would otherwise trigger a rustup network call.
ENV RUSTUP_TOOLCHAIN=1.89.0 \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse \
    CARGO_NET_RETRY=10 \
    CARGO_HTTP_TIMEOUT=600 \
    CARGO_HTTP_MULTIPLEXING=false \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    CARGO_TERM_COLOR=always \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    CARGO_PROFILE_RELEASE_LTO=false \
    RUST_BACKTRACE=1

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates git curl \
 && rm -rf /var/lib/apt/lists/*

# Copy only what the build needs (skip frontend/, target/, .git, etc. via
# .dockerignore) so a code-only edit doesn't bust the apt layer above.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates

# Build with cache mounts; the final `cp` happens inside the same RUN so the
# binary is placed in a non-cached path *before* the cache mount detaches.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    set -eux; \
    cargo build --release -p tn-gateway --locked; \
    cp target/release/tn-gateway /usr/local/bin/tn-gateway

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /usr/local/bin/tn-gateway /usr/local/bin/tn-gateway
ENV BIND=0.0.0.0:9090
EXPOSE 9090
CMD ["tn-gateway"]

