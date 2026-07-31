# Builder: compile a static musl binary on the runner's native architecture.
# rust:alpine targets *-unknown-linux-musl by default, so cargo build emits a
# fully static binary at target/release/.
# Keep in sync with rust-toolchain.toml; bump both together.
# BIN selects the binary: the production server by default, or the demo via
# --build-arg BIN=missing-ebooks-demo (see demo/docker-compose.yml).
ARG BIN=missing-ebooks
FROM rust:1.97.1-alpine AS builder
ARG BIN

# Some crates link a C runtime; musl-dev provides it for the musl target.
RUN apk add --no-cache musl-dev

WORKDIR /build

# assets/ is required: src/web.rs embeds assets/app.css and assets/htmx.min.js
# at compile time via include_str!.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets

RUN cargo build --release --locked --bin "${BIN}"

# Demo runtime: the single demo binary plus a non-root user. /tmp is writable
# for the seeded scenario directory. Built only with `--target demo` and
# BIN=missing-ebooks-demo; a plain `docker build .` never reaches this stage.
FROM alpine:3.24 AS demo
RUN adduser -D -u 1000 demo
COPY --from=builder /build/target/release/missing-ebooks-demo /usr/local/bin/missing-ebooks-demo
USER demo

ENV DEMO_BIND=0.0.0.0:8080 \
    DEMO_SCENARIO=mixed-forest
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/missing-ebooks-demo"]

# Production runtime, deliberately the last stage so an untargeted build (CI
# publish, `docker build .`) produces it: a small Alpine image with just the
# binary and a privilege-drop shim.
FROM alpine:3.24 AS runtime

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
