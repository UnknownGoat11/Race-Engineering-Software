# syntax=docker/dockerfile:1.7
#
# lmu-relay container image (multi-stage, non-root, no shell in the runtime image).
#
# Build from the repository root (the whole Cargo workspace is the build context; only the
# lmu-relay package is compiled):
#
#   docker build -f deploy/Dockerfile -t lmu-relay:latest .
#
# Run: see deploy/docker-compose.yml and deploy/relay.env.example.

# Keep in step with rust-toolchain.toml (the pinned toolchain of the workspace).
ARG RUST_VERSION=1.98.1

FROM rust:${RUST_VERSION}-slim-bookworm AS builder
ARG RUST_VERSION
# Use the image's toolchain as-is (rust-toolchain.toml would otherwise make rustup fetch the
# clippy/rustfmt components, which a release build does not need).
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} \
    CARGO_TERM_COLOR=never
WORKDIR /src
COPY . .
# Cache mounts keep the registry and the target directory between builds.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p lmu-relay \
 && install -D -m 0755 target/release/lmu-relay /out/lmu-relay

# distroless/cc: glibc + libgcc only, no shell or package manager; runs as uid/gid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /out/lmu-relay /usr/local/bin/lmu-relay
USER 65532:65532
# 8443: telemetry (TLS 1.3, ALPN lmure/1) inside the container, published as 443 by compose.
# 9090: admin (/health, /metrics); publish on the host loopback only.
EXPOSE 8443 9090
ENV RELAY_PORT=8443 \
    LOG_FORMAT=json
HEALTHCHECK --interval=15s --timeout=5s --start-period=20s --retries=3 \
    CMD ["/usr/local/bin/lmu-relay", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/lmu-relay"]
CMD ["serve"]
