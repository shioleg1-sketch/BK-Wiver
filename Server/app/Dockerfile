FROM rust:1.94-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY Server/app/Cargo.toml Server/app/Cargo.toml
RUN mkdir -p Server/app/src && echo "fn main() {}" > Server/app/src/main.rs
RUN cargo build -p bk-wiver-server --release

COPY Server/app/src Server/app/src
RUN touch Server/app/src/main.rs
RUN cargo build -p bk-wiver-server --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/bk-wiver-server /usr/local/bin/bk-wiver-server

ENV BK_WIVER_SERVER_ADDR=0.0.0.0:8080
EXPOSE 8080

CMD ["bk-wiver-server"]
