# syntax=docker/dockerfile:1.7

FROM rust:1.96-alpine AS builder
WORKDIR /build
RUN apk add --no-cache musl-dev

# Cache dependencies separately from application source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
COPY migrations ./migrations
RUN touch src/main.rs \
    && cargo build --release --locked \
    && strip target/release/eld-ingest-server

FROM alpine:3.23 AS runtime
RUN apk add --no-cache ca-certificates curl \
    && addgroup --system --gid 10001 eld \
    && adduser --system --uid 10001 --ingroup eld --disabled-password --no-create-home eld

COPY --from=builder /build/target/release/eld-ingest-server /usr/local/bin/eld-ingest-server

USER 10001:10001
EXPOSE 8080
ENV BIND_ADDR=0.0.0.0:8080
ENTRYPOINT ["/usr/local/bin/eld-ingest-server"]
