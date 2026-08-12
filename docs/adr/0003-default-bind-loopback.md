# Default bind address is 127.0.0.1, configurable

Date: 2026-06-04. Amended 2026-08-07 (see Amendment below).

## Context

This deviates from the prevailing homelab/media-app convention of binding all interfaces (`0.0.0.0`/`::`) out of the box (Gitea, Paperless-ngx, Immich, Audiobookshelf). Every one of those ships built-in authentication, which is what makes a broad bind safe. This tool has no auth and an unauthenticated write endpoint (it creates marker files on disk), so it follows the security-first camp instead.

## Decision

The server binds `127.0.0.1` by default, exposes the bind address as a config field with an env override (so it can be set to `0.0.0.0` or a specific interface), and logs a warning at startup when bound to a non-loopback address.

## Consequences

Syncthing's GUI is the direct analog: single-user, no-auth, deliberately bound to loopback as a "reasonably safe default" ([issue #3357](https://github.com/syncthing/syncthing/issues/3357), [guilisten.html](https://docs.syncthing.net/users/guilisten.html)); Miniflux defaults to loopback for the same reason. The *arr stack shows the alternative's cost: it made auth mandatory rather than restrict the bind, and the "disabled for local addresses" relaxation still produced an auth-bypass CVE (GHSA-h5qx-5hjf-7c9r). Loopback also composes with the intended deployment, since Cloudflare Tunnel and `tailscale serve` both reach a localhost origin, so the secure default is also the working default. The Docker image sets `0.0.0.0` explicitly (the native-vs-container divergence Syncthing documents), with exposure controlled at the port-publish layer.

## Amendment (2026-08-07)

The original decision logged a warning and continued when the bind resolved to a non-loopback address. In practice a warning line in startup logs is easy to miss, and the tool's unauthenticated write endpoint means a wrong-interface bind is a real footgun. The startup path now refuses to bind a non-loopback address unless `MISSING_EBOOKS_ALLOW_PUBLIC_BIND=1` is set, exiting with code 1 and naming the flag in the error. Loopback binds are unchanged.

Only the exact string `"1"` opts in. No parsing, no case folding, one truthy value. That keeps the "did the operator acknowledge stepping off loopback" signal grep-legible in deployment configs.

The shipped Docker image sets `MISSING_EBOOKS_ALLOW_PUBLIC_BIND=1` alongside `MISSING_EBOOKS_BIND=0.0.0.0`. The container binds all interfaces on purpose (loopback inside the container would be unreachable from the host), and the exposure control lives at the port-publish layer as described in the original Consequences section. Setting the flag on the image preserves the "bind `0.0.0.0` in the container, publish `127.0.0.1:13379` on the host" pattern with no extra operator action.

Env-only. No config-file field and no CLI flag: the opt-in signal belongs at the deployment layer, not baked into config that travels between machines.
