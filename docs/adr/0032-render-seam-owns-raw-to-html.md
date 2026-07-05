# ADR-0032: render seam owns raw → packaged → HTML

Date: 2026-07-04.

## Context

The render module exposed eight entry points (`package_view`, `package_section`, `render_view`, `render_section`, `roots`, `oob_sections`, `single_oob_section`, `error_section`) plus two public types (`FlaggedView`, `RootSection`). Ten call sites across `web`, `autosync`, and `demo` reconstructed the raw → packaged → HTML walk by hand. Two cross-module contracts leaned on this arrangement: ADR-0022 (cache holds raw scan output, response renders per request) and ADR-0024 (byte-equality between rescan-swap and autosync-push section-level OOB fragments). The autosync module also carried a `section_content_hash` helper that hashed a `RootSection` for dedup, plus a `render_oob_section` free helper that existed only to give the byte-equality test a symbol to compare against.

Candidate #02 of the 2026-07 architecture review flagged the pattern and recommended folding the packaging step inside the render module, along with a per-section handle that owns both the hash and the two render shapes.

## Decision

The render module's outward surface is:

- `render::page(raw, links, mode)` for the full HTML document.
- `render::all_sections(raw, links, mode, wrap)` for the multi-section payload (`SectionWrap::Plain` for the /rescan swap, `SectionWrap::Oob` for the SSE snapshot).
- `render::packaged_section(raw, root, mode)` returning a `SectionHandle` that carries the packaged section plus the identifying root and mode.
- `render::error_section(root, message)` for the standalone bad-root card, unchanged.

`SectionHandle` owns three methods: `content_hash` for autosync dedup, `render(links, alert)` for inline swaps, and `render_oob(links)` for SSE section events. Both render methods route through the same internal per-section renderer, so ADR-0024's byte-equality between rescan-swap and autosync-push is now an internal invariant of the handle.

Everything else in `render.rs` (`FlaggedView`, `RootSection`, `package_view`, `package_section`, `render_view`, `render_section`, `roots`, `oob_sections`, `single_oob_section`) is module-private. `autosync`'s `section_content_hash` and `render_oob_section` free helpers are deleted; their roles are the handle's methods.

The demo overlay's `package_view_with_overlay` now returns the synthesized `RawView` instead of a `FlaggedView`, so the demo handlers call the same seams the production handlers use.

## Consequences

Wins: the render module's outward surface drops from eight entry points plus two types to four entry points plus one small type with three methods. ADR-0024's byte-equality contract lives inside `SectionHandle` by construction rather than being pinned by a cross-module test. Autosync's `compute_pushes` stops owning a hasher. Demo handlers stop echoing the prod render shape. A future per-root field on the packaged section lands in one place.

Costs: one new named concept (`SectionHandle`) plus one enum (`SectionWrap`). `page` and `all_sections` are two entry points where arguably one enum-parameterized function would do; splitting was chosen because `page` also emits the shell, gap summary, and scan bar that the multi-section shapes do not want, and folding those into an enum variant made the internals harder to follow than the split.

Revisit if a future consumer needs the raw packaged section without HTML (a JSON API, a CLI report): the handle grows a `packaged(&self) -> &RootSection` accessor and `RootSection` returns to being crate-public. The seams do not change; only visibility.

## Related

- ADR-0022 (cache holds raw scan output): preserved. The store still owns the raw view; per-request rendering shape is unchanged.
- ADR-0024 (autosync section-level OOB swap): amended. Byte-equality between the two render paths becomes an internal property of `SectionHandle`.
- ADR-0025 (library coverage from per-section data attrs): amended. `package_section` becomes module-internal; future per-root fields still land in one place, now inside the render module.
- ADR-0027 (substrate consolidated behind `RawViewStore`): untouched.
- ADR-0028 (service layer folded into handlers and renderer): extended. That ADR folded the service layer into handlers and the renderer; this ADR continues by pulling packaging inside the renderer.
