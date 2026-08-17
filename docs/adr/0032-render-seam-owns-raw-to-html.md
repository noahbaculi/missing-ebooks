# render seam owns raw → packaged → HTML

> Amended 2026-07-06 by ADR-0034: SSE autosync is removed. `all_sections` no longer takes a wrap parameter, `SectionHandle` no longer exposes `content_hash` or `render_oob`, and the byte-equality contract inside the handle now covers the rescan and refresh paths.

Date: 2026-07-04.

## Context

The render module exposed eight entry points (`package_view`, `package_section`, `render_view`, `render_section`, `roots`, `oob_sections`, `single_oob_section`, `error_section`) plus two public types (`FlaggedView`, `RootSection`). Ten call sites across `web`, `autosync`, and `demo` reconstructed the raw → packaged → HTML walk by hand. Cross-module contracts leaned on this arrangement: ADR-0022 (cache holds raw scan output, response renders per request), and the byte-equality invariant between rescan-swap and refresh-swap section-level fragments. The autosync module also carried a `section_content_hash` helper that hashed a `RootSection` for dedup, plus a `render_oob_section` free helper that existed only to give the byte-equality test a symbol to compare against.

Just before this turn, `src/service.rs` still carried four async wrappers (`current_view`, `rescan`, `mark`, `unmark`), two render helpers, and four types (`FlaggedView`, `RootSection`, `DomainError`, `MarkOutcome`). After the substrate consolidation folded into ADR-0022, each wrapper had collapsed to a three-line pass-through and only `src/web.rs` consumed them. The four types spread one concept across three modules with only one consumer per type.

Candidate #02 of the 2026-07 architecture review flagged the pattern and recommended folding the packaging step inside the render module, along with a per-section handle that owns both the hash and the two render shapes.

## Decision

Two moves land together.

**Service layer dissolved.** The four wrappers inlined into the four handlers in `src/web.rs`. `FlaggedView` and `RootSection` moved next to the markup in `src/web/render.rs`. `DomainError` moved next to the store in `src/state.rs`. `MarkOutcome` dissolved: the `web::mark` handler reads `Applied.created` directly off the store result. The `Arc<FlaggedView>` wrappers vanished; handlers hold `FlaggedView` by value, borrow one `&RootSection` for the section-shaped responses, and drop the view at the end of the response.

**Render module owns raw → packaged → HTML.** The render module's outward surface is:

- `render::page(raw, links, mode, poll_interval_seconds)` for the full HTML document.
- `render::all_sections(raw, links, mode)` for the multi-section payload (`/rescan` and `/refresh`).
- `render::packaged_section(raw, root, mode)` returning a `SectionHandle` for one root.
- `render::error_section(root, message)` for the standalone bad-root card, unchanged.

`SectionHandle` owns one method, `render(links, alert)`, which routes through the internal per-section renderer. The byte-equality invariant now holds between the mark response, the rescan swap, and the refresh swap as an internal property of that renderer.

Everything else in `render.rs` (`FlaggedView`, `RootSection`, `package_view`, `package_section`, `render_view`, `render_section`, `roots`) is module-private.

The demo overlay's `package_view_with_overlay` now returns the synthesized `RawView` instead of a `FlaggedView`, so the demo handlers call the same seams the production handlers use.

## Consequences

Wins: the render module's outward surface drops from eight entry points plus two types to four entry points plus one small type with three methods. The byte-equality contract lives inside `SectionHandle` by construction rather than being pinned by a cross-module test. Autosync's `compute_pushes` stops owning a hasher. Demo handlers stop echoing the prod render shape. A future per-root field on the packaged section lands in one place.

Costs: one new named concept (`SectionHandle`) plus one enum (`SectionWrap`). `page` and `all_sections` are two entry points where arguably one enum-parameterized function would do; splitting was chosen because `page` also emits the shell, gap summary, and scan bar that the multi-section shapes do not want, and folding those into an enum variant made the internals harder to follow than the split.

Revisit if a future consumer needs the raw packaged section without HTML (a JSON API, a CLI report): the handle grows a `packaged(&self) -> &RootSection` accessor and `RootSection` returns to being crate-public. The seams do not change; only visibility. A second HTTP-shaped consumer (JSON API, CLI HTTP harness) would justify lifting the response-packaging pattern back out into a `response` module.

## History

- ADR-0028 (2026-06-24): "service layer folded into handlers and renderer". Folded here as the first move above. Revisit trigger preserved: a second HTTP-shaped consumer would justify lifting the response-packaging pattern back out into a `response` module.

## Related

- ADR-0022 (cache holds raw scan output): preserved. The store still owns the raw view; per-request rendering shape is unchanged.

## Amendment: overlay retired

The demo no longer runs a `MarkOverlay`. Session marks are replayed by folding `raw_view::apply_mark_raw` (the same rule the production write path uses) over the session's `BTreeSet<MarkKey>` at render time. The seam-sharing property this ADR set stays: the demo still hands a synthesized `RawView` to the shared `page`, `packaged_section`, and `all_sections` seams. Mark semantics now live in exactly one function.
