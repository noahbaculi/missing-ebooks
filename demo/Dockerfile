# Builder: compile both binaries against musl on the runner's native arch. On the
# Ampere host this produces arm64 binaries with no cross-compilation.
FROM rust:1.96-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build

# The core crate, needed to build the explore example.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
COPY examples ./examples
RUN cargo build --release --locked --example explore

# The router crate, built as its own workspace.
COPY demo/router ./demo/router
RUN cargo build --release --locked --manifest-path demo/router/Cargo.toml

# Runtime: both binaries plus a non-root user. busybox (in the base) provides the
# shell the router does not need, and /tmp is writable for explore's temp dirs.
FROM alpine:3.21
RUN adduser -D -u 1000 demo
COPY --from=builder /build/target/release/examples/explore /usr/local/bin/explore
COPY --from=builder /build/demo/router/target/release/router /usr/local/bin/router
USER demo

ENV ROUTER_BIND=0.0.0.0:8080 \
    ROUTER_EXPLORE_BIN=/usr/local/bin/explore \
    ROUTER_SCENARIO=mixed-forest
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/router"]
