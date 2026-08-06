# ADR-0022: Raw scan cache, in-place marker writes, and generation-guarded store mutations

Date: 2026-08-04.

## Context

The tool serves gap and show-all views off a scan of every configured library root, and the only mutation the running server performs is a marker click that writes `.no_ebook` or `.ebook_elsewhere` into one folder. Two forces shaped this: the cache had to stay fresh across marker writes without paying for a rewalk on every click, and the substrate that produced the scan (config, scan settings, dir index, cache slot, marker file IO) had to live somewhere a request handler could reach without threading four Arcs per call site.

## Decision

**One cache slot holds raw scan output.** The cache holds one `Option<CacheEntry>` of `Arc<RawView>`, where a `RawView` is one `scanner::RootScan` per configured root (`Walked { canonical_path, folders } | Failed { path, message }`). A request takes the lock, gets or builds the raw view, drops the lock, and renders per `ViewMode` at response time (filter for gaps, then build the tree). The rendered `FlaggedView` is constructed per response and dropped after the response writes.

**Marker writes edit the cache in place.** A mark write removes the marked folder's subtree from the raw view and prunes emptied containers. An undo cannot edit in place (a delete can re-flag a subtree whose pre-mark structure has already been discarded), so it rescans the one affected root and splices the result back into the raw slot. Both leave `stored_at` untouched so the TTL backstop still fires on schedule to pick up out-of-band changes.

**One store owns substrate plus IO.** `RawViewStore` in `src/state.rs` owns the cache slot, the substrate (`Arc<Config>`, `Arc<ScanSettings>`, `Arc<Mutex<DirIndex>>`), and the marker file IO. Its public surface is four async methods: `current`, `refresh`, `rescan`, `write_mark`, `remove_mark`. `AppState` collapses to `{ store, config, autosync }`. `Arc<Config>` is dual-held: the store uses it as scan substrate; `AppState.config` exposes it to handlers that read pure config data (`search_links`, `cookie_name`, `library_roots`).

**Raw-view type and rule live in their own module.** `src/raw_view.rs` owns `RawView`, the pure marker-apply rule (`apply_mark_raw` plus its `add_marker` helper), and the async constructor (`build_view` plus its `build_section` helper). `src/state.rs` owns the store and its lock discipline, the marker file IO, and the coalescer. Every consumer imports from `crate::raw_view::…` directly; `state` does not re-export the moved items.

**Store mutations run in spawned tasks and the slot is generation-guarded.** Each mutation (`write_mark`, `remove_mark`, `rescan`, and the cold build inside `build_coalesced`) runs its side effect plus bookkeeping inside a `tokio::spawn` task that the handler awaits. Once the sequence starts it runs to completion whether or not the request survives. The store's internals live behind one `Arc<StoreInner>` so the spawned closures own what they touch. The cache slot carries a monotonically increasing generation. Every store and in-place edit bumps it. A coalesced build records the generation when it registers; its store is compare-and-store: when the generation moved during the walk, the build's result is served to its awaiters but not persisted. Newest write wins, explicitly, instead of by lock order.

## Consequences

Per-request render cost buys mode-flip latency, cache memory, write-path simplicity, and a smaller cache API. Measured on `mixed-forest` (81 directories across three roots) the worst per-mode render across all roots and modes was 0.086 ms, well under the 2 ms gate; on a synthetic 10k-folder shape sweep the worst (depth, fanout) row was 3.758 ms for gaps and 8.558 ms for show-all, well under the 25 ms gate.

Marker writes stay instant on a large networked library because they edit the cache in place instead of rewalking. Config is immutable at runtime, so `excluded_dirs` and `exclude_globs` need a restart to change; a UI exclude button would require a `RwLock<Config>`, a TOML rewrite path, and invalidate-and-rewalk plumbing per click. Holding config behind a plain `Arc<Config>` with no lock drops all of that and matches the layered env-over-file model (ADR-0004).

A dropped request costs at most one completed unit of work, never a half-applied one, and a completed walk always lands in the cache even when its first requester vanished. A stale build cannot clobber a newer mark or undo. Two concurrent reads on a warm slot both render in parallel and each allocates its own forest; the cold-slot single-flight (lock held across `build`) is preserved.

The trade-offs are one wasted walk when an edit lands mid-build (rare and bounded), and per-render CPU that scales with request count rather than only cache misses. Single-digit ms per render keeps this comfortable. Render memoization is deferred and would be revisited only if the multiplier bit.

## History

This ADR replaces four older records folded here for the first public release. The full text of each stays in git history.

- ADR-0002 (2026-06-22): "marker writes edit the raw cache in place; config is immutable at runtime". Preserved as the in-place edit section above.
- ADR-0027 (2026-06-24): "substrate consolidated behind `RawViewStore`". Preserved as the store surface section above. Two cache-response slots collapsed to one raw slot at 0022, before the store consolidation; before that, the four-Arc clone ceremony threaded through six call sites.
- ADR-0033 (2026-07-05): "raw-view type and rule split out of `state.rs`". Preserved as the raw-view module section above.
- ADR-0036 (2026-07-30): "store mutations run in spawned tasks and the slot is generation-guarded". Preserved as the spawn-and-generation section above. This turn replaced ADR-0002's original "one lock serializes every cache mutation" protocol: a build that started before a mark could overwrite the slot with pre-mark data after the mark completed, and the refreshed `stored_at` kept the stale view alive for a full TTL. The lock could not express the ordering the generation does, so newest-write-wins under generation guard replaced it.

## Related

- ADR-0004: layered env-over-file config. Why an immutable-at-runtime config sidesteps a `RwLock<Config>` and TOML write-back.
- ADR-0008: marker write path guard. The write target is independently canonicalized and confirmed inside a configured root.
- ADR-0019: scan-walk parallelism sized by `scan_concurrency`. Cited for cold-build cost.
- ADR-0020: dir index grows until restart. The store holds the index across scans.
- ADR-0037: request cap and rescan cooldown. Backpressure on the walk that produces the raw view.
