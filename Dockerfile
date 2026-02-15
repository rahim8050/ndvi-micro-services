FROM rust:1.88 AS builder
WORKDIR /app

COPY Cargo.toml ./
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ndvi-service /usr/local/bin/ndvi-service

ENV RUST_LOG=info
EXPOSE 8080
CMD ["ndvi-service"]
