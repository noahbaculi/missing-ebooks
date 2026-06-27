# Builder: compile the demo binary against musl on the runner's native arch. On
# the Ampere host this produces an arm64 binary with no cross-compilation.
FROM rust:1.96-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build

# The whole crate: the demo binary lives in src/bin and reuses the library.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --release --locked --bin missing-ebooks-demo

# Runtime: the single demo binary plus a non-root user. /tmp is writable for the
# seeded scenario directory.
FROM alpine:3.21
RUN adduser -D -u 1000 demo
COPY --from=builder /build/target/release/missing-ebooks-demo /usr/local/bin/missing-ebooks-demo
USER demo

ENV DEMO_BIND=0.0.0.0:8080 \
    DEMO_SCENARIO=mixed-forest
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/missing-ebooks-demo"]
