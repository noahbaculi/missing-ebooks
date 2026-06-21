# ADR-0024: Autosync pushes are per-root section OOB swaps

Date: 2026-06-21.

## Context

The autosync loop (ADR-0023) needs to choose a granularity for the DOM patch it pushes when a root's view changes. Three candidates: per-root section, per-row, or whole-page.

## Decision

Pushes are per-root section, matching the swap unit ADR-0009 already establishes for the Rescan button and the marker handlers. Each `section` SSE event carries one OOB swap fragment with `hx-swap-oob="outerHTML:#root-N-section transition:true"`, which HTMX routes to the targeted `<section>` element. Roots whose rendered HTML did not change since the last broadcast produce no event.

`transition:true` opts into the browser's view-transition API for the swap, so the section fades into its new content rather than blinking. Browsers without view-transition support fall back to a plain swap. No correctness risk.

## Consequences

A change deep inside a section (one new folder in a large author) re-renders the whole section and replaces the DOM node. On the reference scenarios this is microseconds and bytes; on a future flagship library with thousands of folders per section it may become measurable. Per-row OOB swaps are the deferred next step if this hurts in practice: they require stable per-folder DOM ids, add/remove plumbing for insertions and deletions, and a richer event protocol. None of that earns its complexity at v1.

The byte-equal invariant tested by `tests/cache_render_byte_equal.rs` extends to cover SSE payloads: the section a tab receives via SSE equals the section it would get from clicking Rescan.

## Alternatives considered

- **Per-row OOB swap**: minimal DOM churn but a real refactor; revisit if real workloads show section-level swaps cause noticeable jank.
- **Push a "rescanned" event and let the client GET /**: simplest server but defeats the point because the client does the work and the page flickers.
- **Push every section every tick**: see ADR-0023.
