# ADR-0024: Autosync pushes are per-root section OOB swaps

> Amended 2026-06-25 by ADR-0030: "snapshot before subscribe" narrows to "ack before subscribe, snapshot before subscribe when sent." The per-section seed hashes still feed `subscribe_and_seed` in both branches; loop-level first-tick dedup is unaffected.

Date: 2026-06-21.

## Context

The autosync loop (ADR-0023) needs to choose a granularity for the DOM patch it pushes when a root's view changes. Three candidates: per-root section, per-row, or whole-page.

## Decision

Pushes are per-root section, matching the swap unit ADR-0009 already establishes for the Rescan button and the marker handlers. Each `section` SSE event carries one OOB swap fragment with `hx-swap-oob="outerHTML:#root-N-section"`, which HTMX routes to the targeted `<section>` element. Roots whose rendered HTML did not change since the last broadcast produce no event.

The attribute stays as `<style>:<id-selector>` with nothing after the selector. htmx 2.x's OOB parser (`He` in `htmx.min.js`) splits the attribute value on the *first* colon: everything before is the swap style, everything after is the CSS selector. Any `hx-swap` modifier with a colon ("transition:true", "swap:200ms", and friends) inside the OOB attribute lands in the selector portion and silently breaks routing. An earlier revision of this ADR appended `transition:true`; the section-event swaps never reached the DOM until that token was removed (commit `13815f8`). `src/web/render.rs::tests::single_oob_section_attribute_survives_htmx_first_colon_parse` now pins the shape.

A section fade on swap is desirable but cannot live inside `hx-swap-oob`. The route to bring it back is `htmx.config.globalViewTransitions = true`, which applies to every swap on the page (Rescan and marker handlers included). That scope expansion is its own decision and belongs in a separate ADR.

## Consequences

A change deep inside a section (one new folder in a large author) re-renders the whole section and replaces the DOM node. On the reference scenarios this is microseconds and bytes; on a future flagship library with thousands of folders per section it may become measurable. Per-row OOB swaps are the deferred next step if this hurts in practice: they require stable per-folder DOM ids, add/remove plumbing for insertions and deletions, and a richer event protocol. None of that earns its complexity at v1.

The byte-equal invariant tested by `tests/cache_render_byte_equal.rs` extends to cover SSE payloads: the section a tab receives via SSE equals the section it would get from clicking Rescan.

The per-broadcast dedup is computed from the packaged section (`web::render::RootSection` via its derived `Hash`), not the rendered HTML. The hash is per-mode because `package_section` discards inputs the mode does not render (e.g. show-all-only `cover_files` on a covered audiobook collapse out of the gaps `RootState`), and a mode-independent hash on the raw `RootScan` would push to subscribers whose view is byte-equal across the change. Equality of the per-mode content hash implies equality of the rendered HTML for that mode because the renderer is pure on `(packaged_section, root_idx, mode, links)` and the three non-section inputs are loop-stable for one autosync registry. The `content_hash_equals_render_parity` test in `src/autosync.rs::tests` pins this contract; `gaps_hash_unchanged_when_show_all_only_change_lands` pins the per-mode isolation; `no_change_tick_does_not_render` pins the loop-level render-avoidance using a test-only `render_count` accessor on `Autosync`.

## Alternatives considered

- **Per-row OOB swap**: minimal DOM churn but a real refactor; revisit if real workloads show section-level swaps cause noticeable jank.
- **Push a "rescanned" event and let the client GET /**: simplest server but defeats the point because the client does the work and the page flickers.
- **Push every section every tick**: see ADR-0023.
