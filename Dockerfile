FROM rust:1.88 AS chef
WORKDIR /app
RUN cargo install --locked cargo-chef

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates/common/Cargo.toml crates/common/Cargo.toml
COPY services/ndvi/Cargo.toml services/ndvi/Cargo.toml
COPY services/weather/Cargo.toml services/weather/Cargo.toml
COPY src/main.rs src/main.rs
COPY crates/common/src/lib.rs crates/common/src/lib.rs
COPY services/ndvi/src/lib.rs services/ndvi/src/lib.rs
COPY services/weather/src/main.rs services/weather/src/main.rs
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY services ./services
COPY src ./src
RUN cargo build --release -p ndvi-service --bin ndvi-service

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ndvi-service /usr/local/bin/ndvi-service

ENV RUST_LOG=info
EXPOSE 8081
CMD ["ndvi-service"]
