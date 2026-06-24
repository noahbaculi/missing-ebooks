# Substrate consolidated behind RawViewStore

The scan substrate (config, scan settings, dir index, cache slot) and the marker file IO used to live in three places. `Cache` in `src/state.rs` held the slot and four closure-taking primitives (`get_or_build`, `rebuild`, `apply_marker_or_build`, `rebuild_root`). `AppState` held the substrate Arcs. `src/service.rs` held the marker IO plus the four-Arc clone ceremony that fed every primitive its `F: FnOnce() -> Fut` build closure. `service::mark` and `service::unmark` were about forty lines each, mostly `Arc::clone` calls. The method `apply_marker_or_build` named a marker concern from inside the cache layer without doing the marker work itself.

A single `RawViewStore` in `src/state.rs` now owns the slot, the substrate (`Arc<Config>`, `Arc<ScanSettings>`, `Arc<Mutex<DirIndex>>`), and the marker file IO. Its public surface is four async methods: `current`, `refresh`, `rescan`, `write_mark`, `remove_mark`. `AppState` collapses to `{ store, config, autosync }`. The `Arc<Config>` is dual-held: the store uses it as scan substrate (driving `build_view`), and `AppState.config` exposes it to handlers that read pure config data (`search_links`, `cookie_name`, `library_roots`). Both clone the same Arc, not two copies. The four `Cache` primitives and their closure-taking generics are gone; lock-check-build-store is concrete code inside each store method. `write_marker`, `delete_marker`, `apply_mark_raw`, `add_marker`, and `invalidate_index` moved from `service.rs` into `state.rs` as private helpers.

`service::mark` and `service::unmark` are now three-line wrappers that delegate to the store and render. The four-Arc clone ceremony at six call sites (four in `service.rs`, one each in `web.rs` and `autosync.rs`) is gone. The ADR-0002 invariant (marker write plus dir-index invalidate plus in-place cache edit are one operation) lives in one place: `RawViewStore::write_mark`. Test mechanics that previously asserted on `Cache` moved to `RawViewStore`; the `Arc::ptr_eq` slot-identity checks survive via `#[cfg(test)] peek_stored_arc()` on the store.

Alternatives we set aside. Keeping `Cache` and only consolidating the IO into a thin wrapper would leave the closure-taking generics and the four-Arc ceremony in place. Moving all of `Config` behind the store would force every handler that reads `state.config.search_links` to go through `state.store.config()`; the win was concentrating the *substrate* role of config, not policing pure-data reads, and dual-holding the same Arc costs nothing. Splitting the store into separate cache and IO modules would re-introduce the cross-module ordering invariant the consolidation exists to eliminate.

Revisit if a future API surface (a JSON read endpoint, a CLI scan tool) needs raw scan output without the marker IO, since at that point the store's two responsibilities pull in different directions and might justify the split this ADR rejected.

## Related

- ADR-0002: marker writes edit cache in place. Preserved; now implemented inside `write_mark`.
- ADR-0020: dir index grows until restart. Preserved; the store holds the index.
- ADR-0022: cache holds raw scan output. Preserved; the store is the cache.
