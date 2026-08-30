FROM rust:1.93-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY src src
COPY migrations migrations

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /build/target/release/matrix-service /usr/local/bin/matrix-service

ENV ROCKET_ADDRESS=0.0.0.0
ENV ROCKET_PORT=8787

EXPOSE 8787

CMD ["matrix-service"]
