# Multi-stage production build for TrustLedger services
FROM rust:1.80-slim-bullseye AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy workspace manifests
COPY Cargo.toml Cargo.lock ./
COPY crates/ledger-core crates/ledger-core
COPY crates/wal crates/wal
COPY crates/ingest crates/ingest
COPY crates/raft crates/raft
COPY crates/merkle crates/merkle
COPY crates/solana-settle crates/solana-settle
COPY crates/observability crates/observability
COPY apps/demo apps/demo
COPY simulator simulator

# Build release binaries
RUN cargo build --release -p demo -p simulator

# Runtime container
FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/demo /usr/local/bin/trustledger-demo
COPY --from=builder /app/target/release/simulator /usr/local/bin/trustledger-simulator

# Unprivileged user for security
RUN useradd -m -u 10001 ledger && chown -R ledger:ledger /app
USER ledger

EXPOSE 8080 9090

ENTRYPOINT ["/usr/local/bin/trustledger-demo"]
