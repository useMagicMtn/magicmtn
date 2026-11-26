# Static musl binaries run in both Alpine sandboxes and glibc distributions.
# Cross-compile on Apple Silicon without emulating the Rust compiler.
FROM docker.io/library/rust:1.100.0-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea AS builder
RUN apt-get update && apt-get install -y --no-install-recommends python3-venv \
    && rm -rf /var/lib/apt/lists/*
RUN python3 -m venv /opt/cross \
    && /opt/cross/bin/pip install --no-cache-dir cargo-zigbuild==0.23.4 ziglang==0.14.1
ENV PATH=/opt/cross/bin:$PATH
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY release ./release
COPY cli ./cli
COPY core ./core
COPY web-ui ./web-ui
COPY local ./local
COPY assets ./assets
COPY runtime ./runtime
COPY registry ./registry
COPY scripts/install-public.sh ./scripts/install-public.sh
COPY tests ./tests
COPY interfaces/pi ./interfaces/pi
COPY interfaces/opencode ./interfaces/opencode
COPY sdk/rust ./sdk/rust
RUN cargo zigbuild --release --locked --no-default-features -p magicmtn-cli --bin magic \
    --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl \
    && mkdir /out \
    && cp target/x86_64-unknown-linux-musl/release/magic /out/magic-linux-x86_64 \
    && cp target/aarch64-unknown-linux-musl/release/magic /out/magic-linux-arm64 \
    && ! readelf -l /out/magic-linux-x86_64 /out/magic-linux-arm64 | grep 'INTERP' \
    && ! readelf -d /out/magic-linux-x86_64 /out/magic-linux-arm64 | grep 'NEEDED'

FROM scratch
COPY --from=builder /out/ /
