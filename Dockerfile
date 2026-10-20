# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.99-bookworm
ARG ZIG_VERSION=0.17.0
ARG ZIGBUILD_VERSION=0.23.4

FROM --platform=linux/amd64 rust:${RUST_VERSION} AS base

ARG ZIG_VERSION
ARG ZIGBUILD_VERSION

ENV CARGO_TERM_COLOR=always

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-x86_64-linux-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz \
    && tar -C /opt -xf /tmp/zig.tar.xz \
    && mv "/opt/zig-x86_64-linux-${ZIG_VERSION}" /opt/zig \
    && ln -s /opt/zig/zig /usr/local/bin/zig \
    && rm /tmp/zig.tar.xz

RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
    && rustup component add rustfmt clippy \
    && cargo install cargo-zigbuild --locked --version "${ZIGBUILD_VERSION}"

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests

FROM base AS test
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo fmt --check \
    && cargo clippy --all-targets -- -D warnings \
    && cargo test --all-targets

FROM base AS builder
ARG TARGETARCH=amd64

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo zigbuild --release --locked --target x86_64-unknown-linux-musl \
    && cargo zigbuild --release --locked --target aarch64-unknown-linux-musl \
    && mkdir -p /out \
    && cp target/x86_64-unknown-linux-musl/release/simplebot /out/simplebot-linux-amd64 \
    && cp target/aarch64-unknown-linux-musl/release/simplebot /out/simplebot-linux-arm64 \
    && cp "/out/simplebot-linux-${TARGETARCH}" /out/simplebot

FROM scratch AS artifacts
COPY --from=builder /out/simplebot-linux-amd64 /out/simplebot-linux-amd64
COPY --from=builder /out/simplebot-linux-arm64 /out/simplebot-linux-arm64

FROM ubuntu:24.04 AS runtime

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        dbus \
        file \
        iproute2 \
        iputils-ping \
        jq \
        less \
        nano \
        netplan.io \
        network-manager \
        procps \
        rfkill \
        systemd \
        util-linux \
        wpasupplicant \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /out/simplebot /usr/local/bin/simplebot
COPY config/config.toml.example /etc/simplebot/config.toml.example
COPY examples/skills /etc/simplebot/skills

RUN chmod 755 /usr/local/bin/simplebot \
    && chmod 644 /etc/simplebot/config.toml.example

WORKDIR /root
ENTRYPOINT ["/usr/local/bin/simplebot"]
