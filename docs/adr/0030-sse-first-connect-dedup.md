# ADR-0030: SSE first connect skips the snapshot

Date: 2026-06-25.

## Context

`/events` sends a `snapshot` event as the first emission on every connection (`src/web.rs:212-219`, `src/autosync.rs:136-171`). The snapshot is byte-identical to what the page just rendered inline. The browser parses it and `htmx-sse` performs one OOB swap per root, every one a no-op against an identical existing DOM. Headless-Chrome profiling against `examples/explore.rs mixed-forest` puts the cost at 126 ms in a single long task on the main thread, landing right after `load`. Production and demo both pay it on every visit.

The snapshot exists for a real reason. The autosync loop (ADR-0023) may tick between page render and SSE connect. Without the snapshot, a subscriber would render an inline view that the autosync loop already moved past, with no recovery until the next tick.

## Decision

The server skips the snapshot when the client is on a first connect, and sends it when the client is on a reconnect. The discriminator is the standard `Last-Event-ID` request header.

For the header to be present on reconnects, every event the server emits carries an `id:` field. The value is the literal `"r"`. The server never reads the value back, so it carries no semantics, only presence. The first emission on every connection is an `ack` event with `event: ack`, `id: r`, empty data. No client listens to `ack`. Its sole job is to seed the browser's `lastEventId` before the connection has a chance to drop, so the first reconnect after a fresh page is detectable.

`Last-Event-ID` absent on `/events` means first connect: send the `ack`, skip the snapshot, subscribe with the per-section seed hashes from `snapshot_and_seed` so the autosync loop's first tick suppresses redundant sections (ADR-0024). `Last-Event-ID` present means reconnect: send the `ack`, send the snapshot, subscribe.

The same shape lives in both `src/web.rs::events` (production, autosync-backed) and `src/demo/handlers.rs::events` (demo, session-derived). `ack_event`, `snapshot_event`, and `section_event` factories live next to `events_response` in `src/web.rs` so the wire stamp is set in one place.

## Consequences

Cold page load drops ~126 ms of post-load main-thread work on the common path. The autosync loop is unchanged: it still hashes per packaged section, still ticks on the configured interval, still pushes only changed sections. Subscribe with seed hashes runs in both branches, so a tick that fires immediately after a first-connect subscribe does not redundantly broadcast the inline-rendered state.

A connection that drops between the server sending `ack` and the browser receiving it reconnects without `Last-Event-ID` and is treated as a first connect. The window is microseconds. The accepted failure mode is identical to a real first connect: the client trusts the inline render, the autosync loop reconciles on the next tick. Staleness bound is one `autosync_interval_seconds`. The default in production is 10 s. The demo runs with interval 0 and has no autosync ticks at all, so the question reduces to "the page render is the truth," which is correct for a static seeded library and per-session marks.

The `id: "r"` stamp is harmless to consumers that ignore it, which is every current consumer. Adding the stamp to every event also unblocks any future event-replay protocol that wants to keep the same shape.

ADR-0024's "snapshot before subscribe" invariant relaxes to "ack before subscribe, and snapshot before subscribe when sent". The relaxation is recorded in a one-paragraph amendment to ADR-0024 rather than in this ADR.

## Alternatives considered

- **Per-section seen-hash protocol.** The inline page emits a hash per section. The client sends them on `/events?seen=`. The server diffs against the current state hash and omits any section whose hash matches. More correct, larger surface, reuses the per-section hashing the autosync loop already does. Rejected as the first step: it changes the page markup and the wire shape in tandem, and the simpler discriminator gets the same first-connect savings with no markup change. Worth revisiting if the accepted race causes real problems.
- **Client-side `htmx:beforeSwap` dedup.** A listener in `app.js` compares an incoming OOB section to the matching live section and cancels the swap when they match. No server change. The snapshot bytes still cross the wire and `htmx-sse` still parses them (~30 ms of the 126 ms long task remains), but the expensive swap step is skipped. Rejected as the primary fix: smaller save, and `Last-Event-ID` is the cleaner contract. Reasonable to layer on top later as defense in depth.
- **Per-connection cookie discriminator.** Server mints a cookie on first connect; reconnect carries it. Avoids touching event ids. Rejected because `Last-Event-ID` is the standard SSE mechanism for "have you seen events from me before", and the `ack` sentinel pattern is well understood. A new cookie is more surface to maintain.
- **Always skip the snapshot.** Cleanest wire, no discriminator. Rejected because a reconnect after a real connection drop genuinely needs the snapshot: the autosync loop's `last_content_hash` is per-mode, not per-subscriber, so a reconnecting subscriber can be stuck on stale state until the next ticked change.
