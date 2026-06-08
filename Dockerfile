# Builder: compile a static musl binary on the runner's native architecture.
# rust:alpine targets *-unknown-linux-musl by default, so cargo build emits a
# fully static binary at target/release/.
FROM rust:1.96-alpine AS builder

# Some crates link a C runtime; musl-dev provides it for the musl target.
RUN apk add --no-cache musl-dev

WORKDIR /build

# assets/ is required: src/web.rs embeds assets/app.css and assets/htmx.min.js
# at compile time via include_str!. examples/ is required because the Cargo
# manifest declares the explore example target and Cargo validates its path.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
COPY examples ./examples

RUN cargo build --release --locked --bin missing-ebooks

# Runtime: a small Alpine image with just the binary and a privilege-drop shim.
FROM alpine:3.21

# su-exec drops root to the configured PUID/PGID in the entrypoint. busybox
# wget (already in the base) backs the healthcheck.
RUN apk add --no-cache su-exec

COPY --from=builder /build/target/release/missing-ebooks /usr/local/bin/missing-ebooks
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Bind all interfaces inside the container (loopback would be unreachable from
# the host). Exposure is controlled at the port-publish layer (see ADR 0003).
ENV MISSING_EBOOKS_BIND=0.0.0.0 \
    PUID=1000 \
    PGID=1000

EXPOSE 13379

# Honor a custom port if MISSING_EBOOKS_PORT is set; default to 13379.
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD wget -q -O /dev/null "http://127.0.0.1:${MISSING_EBOOKS_PORT:-13379}/" || exit 1

ENTRYPOINT ["/entrypoint.sh"]
