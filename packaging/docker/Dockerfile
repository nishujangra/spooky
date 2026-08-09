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
COPY spooky ./spooky

RUN cargo build --release --locked -p spooky --bin spooky

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --system --uid 10001 --create-home --home-dir /var/lib/spooky spooky

WORKDIR /var/lib/spooky

COPY --from=builder /workspace/target/release/spooky /usr/local/bin/spooky

RUN mkdir -p /etc/spooky /etc/spooky/certs /var/log/spooky \
    && chown -R spooky:spooky /etc/spooky /var/log/spooky /var/lib/spooky

EXPOSE 9889/udp
EXPOSE 9889/tcp
EXPOSE 9901/tcp
EXPOSE 9902/tcp

USER spooky

ENTRYPOINT ["/usr/local/bin/spooky"]
CMD ["--config", "/etc/spooky/config.yaml"]
