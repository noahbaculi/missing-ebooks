# ADR-0025: Library coverage is derived from per-section data attributes

Date: 2026-06-22.

## Context

The gap-summary strip needs a library coverage readout (covered audiobooks over total audiobooks across every successfully-scanned root) that stays current across marks, rescans, and autosync section pushes. Three candidates for where the numbers live: a server-rebuilt strip pushed as a new OOB target on every change, a scanner-side stat stored alongside the raw vec, or a per-section data attribute the client aggregates.

## Decision

The total audiobook count per root rides to the browser as `data-total-audiobooks` on each `<section class="root">`. `app.js` sums the attribute across sections and derives `covered = total - currentGapTotal()` on the same `marked` and `htmx:afterSwap` hooks the prior session bar already used. Errored sections carry `data-total-audiobooks="0"` so they fold out of the sum without a special case.

The strip carries two readouts that `app.js` toggles between on the same recompute. The head holds `{pct}% covered · {covered} of {total} audiobooks` with the progress bar when gaps remain; the all-clear line carries a trailing ` · 100% covered ({T} of {T} audiobooks)` fragment when the library has audiobooks but no gaps, and stays bare when the library is empty (so the line never reads "0 of 0"). The all-clear tail's two numeric values ride in their own child spans so `app.js` only rewrites the digits and the surrounding wording lives once in the server template. The head readout floors the percent so 199 of 200 reads "99% covered" next to a hero "1 gap to fill", never a false "100%" while gaps remain.

A small `service::count_audiobooks(&RawRootState) -> usize` helper filters the raw `Vec<ScannedFolder>` already in the cache (ADR-0022). One `service::render_section_from_raw(raw, mode)` packages a raw section with its rendered state and audiobook total; both `render_view` (snapshot path) and `autosync::render_oob_section` (push path) call it, so any future per-root field lands in one place. `render_section` emits the attribute on the section open tag.

## Consequences

The coverage readout rides every existing swap channel: a mark replaces the closest section (the per-root total is invariant within a scan, so it rides along unchanged), a rescan swaps `#roots`, an autosync push swaps one section. No new OOB target, no new event type, no autosync protocol bump.

The cost is one filter over the raw vec per render per root. On `mixed-forest` (81 folders across three roots) this is negligible; on a 10k-folder library it is one pass over 10k entries, well under the 25 ms render gate ADR-0022 measured.

The pattern matches the chip and hero updaters, which also count off the DOM. Future per-root coverage on the chips, if it lands, reuses the same data attribute. A future per-root field on `RootSection` (per-root coverage, last-scanned timestamp) only has to thread through `render_section_from_raw`.

## Alternatives considered

- **Server-rebuilt strip with a new OOB target**: adds an OOB target on every mark, undo, rescan, and autosync push; the strip becomes a swap surface the protocol has to keep in sync. The data-attribute path needs none of that.
- **Scanner-side stat on `RawRootState`**: bakes a derived statistic into the cache. The vec already has the fact; counting it at render time is `O(folders)` and stays in the layer that already renders.
- **No live updates, paint-once readout**: a stale percent after every mark; defeats the point of the readout.
- **Round the percent instead of floor**: reads "100% covered" beside "1 gap to fill" at high coverage. The floor never lies, at the cost of "0%" while one of a thousand audiobooks is covered.
