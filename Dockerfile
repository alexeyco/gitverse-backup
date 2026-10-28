FROM rust:1.99.1-bookworm AS builder
WORKDIR /build
COPY Cargo.toml ./
COPY Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/gitverse-backup /usr/local/bin/gitverse-backup
ENTRYPOINT ["/usr/local/bin/gitverse-backup"]
