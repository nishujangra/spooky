FROM rust:1-bookworm AS builder

WORKDIR /workspace

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang \
        cmake \
        libclang-dev \
        ninja-build \
        pkg-config \
        perl \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY impulse ./impulse

RUN cargo build --release --locked -p impulse --bin impulse

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --system --uid 10001 --create-home --home-dir /var/lib/impulse impulse

WORKDIR /var/lib/impulse

COPY --from=builder /workspace/target/release/impulse /usr/local/bin/impulse

RUN mkdir -p /etc/impulse /etc/impulse/certs /var/log/impulse \
    && chown -R impulse:impulse /etc/impulse /var/log/impulse /var/lib/impulse

EXPOSE 9889/udp
EXPOSE 9889/tcp
EXPOSE 9901/tcp
EXPOSE 9902/tcp

USER impulse

ENTRYPOINT ["/usr/local/bin/impulse"]
CMD ["--config", "/etc/impulse/config.yaml"]
