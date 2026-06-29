# ADR-0023: Autosync only runs while a browser is subscribed

Date: 2026-06-21.

## Context

Before this decision, the Rescan button was the only way to refresh a page; viewers with a tab open against a library that changed externally saw stale data until they clicked. The `ttl_seconds` backstop refreshes on the next page load, not while a tab sits open. A long-running self-hosted server holding a few sourced ebooks per day calls for live updates without polling on a quiet library.

## Decision

A single process-wide background task ("autosync") runs whenever at least one SSE subscriber is connected. The first subscriber spawns the task; the last unsub lets it exit. Each tick rebuilds the render cache through `Cache::rebuild` (single-flighted with the manual Rescan and TTL paths), diffs each rendered section against the last broadcast, and pushes one OOB-swap fragment per changed `(mode, root)` over SSE. Idle gap: the timer measures from scan completion to next start.

The `incremental_scan` config knob is removed: warm scans become the default for every scan path (cold-cache page load, TTL-expiry rebuild, autosync tick). The dir index becomes process-lifetime state read by every scan, discarded only on a `/rescan` click or process restart (see ADR-0020). `/rescan` becomes the explicit cold-scan path: it clears the dir index, then walks every directory.

`autosync_interval_seconds` (default 10) governs the cadence. Setting it to `0` disables the loop entirely; the SSE endpoint still serves the initial snapshot but emits no further section events.

## Consequences

The Rescan button on SMB moves from ~260 ms (warm scan today) to ~1.9 s (cold scan). The trade buys unambiguous "force-clean" semantics: clicking Rescan is now the recovery path for any drift, and routine refresh happens via autosync without a click.

A reverse proxy needs to allow SSE: long-lived connection, `text/event-stream`, no buffering on the autosync path. The KeepAlive pings every 15 s survive idle TCP drops by default.

`ttl_seconds` keeps its role as the between-session staleness backstop: while a tab is subscribed, autosync rebuilds the cache far more often than the TTL would expire it. When the last tab closes, TTL is what governs how stale the cache can be when the next visitor arrives.

The demo binary disables autosync per session (`autosync_interval_seconds = 0`) because the session sweep's idle signal does not yet track SSE traffic.

## Alternatives considered

- **Periodic polling**: simpler transport but always-on traffic and per-tab latency tied to the poll interval. SSE is one connection per tab with push semantics.
- **WebSocket**: bidirectional, unneeded here (the server has nothing to ask the client).
- **Push every section every tick**: simpler server but DOM churn and wire traffic on a quiet library. The per-root diff has zero traffic on a quiet library after the snapshot.
- **One loop per view mode** or **per subscriber**: doubles or N-multiplies the scan cost. One process-wide loop renders both modes from one raw scan; rendering is microseconds.
