# syntax=docker/dockerfile:1.6 

ARG BIN=trait-test-services

FROM rust:1.81 AS builder
ARG BIN
WORKDIR /app

RUN <<'EOSH'
  mkdir -p /usr/local/cargo /app/target
EOSH

ENV CARGO_HOME=/usr/local/cargo

COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build -p test-services || true

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build -p test-services && cp /app/target/debug/${BIN} /app/${BIN}

###################################

FROM debian:bookworm-slim
ARG BIN

COPY --from=builder /app/${BIN} /bin/server

ENV RUST_LOG="debug,restate_shared_core=trace" \
    RUST_BACKTRACE=1

CMD ["/bin/server"]
