# ADR-0034: Refresh is a client-driven poll of `GET /refresh`

Date: 2026-07-06.

## Context

ADR-0023 introduced a server-side background loop that pushed changed sections over SSE while a browser was subscribed. ADR-0024 fixed the push granularity at one root section per event. ADR-0030 optimized cold load by discriminating first connect from reconnect with `Last-Event-ID`. The three ADRs together were the single largest source of complexity in the codebase: `src/autosync.rs` alone was 964 lines, on top of an SSE endpoint with a two-branch handshake, per-mode subscriber registries, per-root content hashes, an `ack` sentinel, and dependencies (`enum-map`, `tokio-stream`) that existed to serve this feature.

The user need is real: a self-hoster keeps the dashboard open while dropping ebooks into folders from a file manager, and the dashboard should reflect the drops without a manual reload. A design review on 2026-07-04 questioned whether server push was the simplest way to hit that need, and whether SSE was earning its complexity for the deployment shape this tool targets (one to two self-hosted tabs, warm scans in the low milliseconds).

## Decision

The dashboard refreshes by polling. Every open tab hits `GET /refresh?view=<mode>` on an interval driven by `poll_interval_seconds` (default 10 s), gated on `document.visibilityState === "visible"` with a `visibilitychange` listener that fires one immediate poll when the tab returns to focus. The endpoint reads from `RawViewStore::current()`, so `ttl_seconds` (default lowered to 10 s) caps how often the underlying scan runs regardless of tab count. `build_coalesced` still joins concurrent cold builds into one walk.

The response is byte-equal to the `/rescan` response for the same underlying raw view and `view=` mode: both route through `SectionHandle` per ADR-0032, both target `#roots` with `innerHTML` swap. The one behavioral difference is that `/refresh` does not send `HX-Push-Url`; a poll is not a navigation.

The SSE endpoint, the subscriber registry, the per-tick loop, the snapshot handshake, the `ack` sentinel, and the `htmx-sse` vendored asset are removed. `AppState` loses its `autosync` field. `src/autosync.rs` is deleted. `enum-map` and `tokio-stream` drop out of `Cargo.toml`. `autosync_interval_seconds` and its env override drop out of `Config`.

## Consequences

The idle wire cost pattern flips. Long-lived SSE on a quiet library was one 15 s keepalive per tab (~4 KB per tab per hour). A 10 s poll with request headers is roughly 170 KB per tab per hour while visible, zero while backgrounded. On a LAN self-host case this is invisible; on a metered remote deployment a user can dial `poll_interval_seconds` up.

Server-side scan rate is bounded by `ttl_seconds`, not tab count. Twenty tabs polling every 10 s with `ttl_seconds = 10` and a sub-second warm scan run at most one scan per interval regardless. Tabs polling faster than TTL do not increase server work; only their own network cost.

The delete removes the test-only observability seams that existed to make the loop safe (`subscriber_count`, `render_count`, `abort_loop_for_test`, `has_seeded_baseline_for_test`), the poison-recovery pattern on the registry mutex, the `Weak<AppState>` lifecycle glue, and the per-mode hash bookkeeping. Coverage that evaporates: no-change-tick render-avoidance, per-mode subscriber isolation, registry poison recovery. Those properties existed to make the loop safe; without the loop they have no target.

`MISSING_EBOOKS_AUTOSYNC_INTERVAL_SECONDS`, if set in an existing deployment, becomes a silent no-op. Pre-release, so no compat shim.

The connection banner in `assets/app.js` stays as is. It already covers `/mark` and `/rescan` retries via `htmx:sendError | timeout | responseError`. A polled `/refresh` failing is transient and the next tick recovers; surfacing every fail would be noisier than useful.

## Related

- ADR-0023 (autosync only runs while subscribed): superseded.
- ADR-0024 (autosync section-level OOB swap): superseded. The section is still the swap unit for `/refresh`, but the transport is a plain HTMX GET response, not an SSE OOB fragment. The byte-equal invariant between the rescan swap and the refresh swap holds through `SectionHandle` per ADR-0032.
- ADR-0030 (SSE first-connect dedup): superseded. No snapshot event, no `Last-Event-ID` discriminator, no `ack` sentinel.
- ADR-0002 (marker writes edit cache in place): unchanged.
- ADR-0022 (raw cache + render per request): unchanged. `current()` is exactly this contract.
- ADR-0027 (substrate consolidated behind `RawViewStore`): unchanged.
- ADR-0032 (render seam owns raw to HTML): unchanged. `/refresh` renders through `SectionHandle` like `/rescan`.
