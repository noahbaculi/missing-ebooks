# Store mutations run in spawned tasks and the slot is generation-guarded

Date: 2026-07-30.

## Context

axum drops a handler future when its client disconnects. Every store mutation (`write_mark`, `remove_mark`, `rescan`, and the cold build inside `build_coalesced`) used to run its side effect and its bookkeeping inline in the handler future, so an abort could split a marker write or delete from its index invalidation and cache edit, strand cleared indices behind an aborted rescan, or discard a completed walk whose owning request vanished. Separately, ADR-0027's microsecond-lock rework moved the build's store outside the inflight lock without revisiting ADR-0002's claim that one lock serializes every cache mutation, so a build that started before a mark could overwrite the slot with pre-mark data after the mark completed, and the refreshed `stored_at` kept the stale view alive for a full TTL.

## Decision

Each mutation runs its side-effect-plus-bookkeeping sequence inside a `tokio::spawn` task that the handler merely awaits. Once the sequence starts it runs to completion whether or not the request survives. The store's internals live behind one `Arc<StoreInner>` so the spawned closures own what they touch.

The cache slot carries a monotonically increasing generation. Every store and every in-place edit bumps it. A coalesced build records the generation when it registers and its store is compare-and-store: when the generation moved during the walk, the build's result is served to its awaiters but not persisted. Newest write wins, explicitly, instead of by lock order.

## Consequences

A dropped request costs at most one completed unit of work, never a half-applied one, and a completed walk always lands in the cache even when its first requester vanished. A stale build can no longer clobber a newer mark or undo. The trade-off is one wasted walk when an edit lands mid-build, which is rare and bounded. Serializing `store_fresh` under the inflight lock was rejected during triage: a build that started before a mark still overwrites the slot with pre-mark data after the mark completes, so the lock cannot express the ordering the generation does. ADR-0002 and ADR-0027 carry amendment notes pointing here.
