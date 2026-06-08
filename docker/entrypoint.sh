#!/bin/sh
# Container entrypoint: pick the run-as user, optionally layer in a mounted
# config file, then drop privileges and launch the server.
#
# Marker files the server writes into the mounted library are owned by this
# user, so PUID/PGID should match whoever owns the library on the host.
set -eu

PUID="${PUID:-1000}"
PGID="${PGID:-1000}"

# Conventional path for a mounted config.toml. Overridable (mainly for tests).
CONFIG_FILE="${CONFIG_FILE:-/config/config.toml}"

# If a config file is mounted and the caller did not already pass --config,
# layer it in. Env vars still override file values (see ADR 0004).
if [ -f "$CONFIG_FILE" ]; then
  case " $* " in
    *" --config "*|*" --config="*) ;;
    *) set -- --config "$CONFIG_FILE" "$@" ;;
  esac
fi

# su-exec accepts a numeric uid:gid, so no user account needs to exist in the
# image. exec replaces this shell so signals reach the server directly.
exec su-exec "${PUID}:${PGID}" missing-ebooks "$@"
