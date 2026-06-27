# Service layer folded into handlers and renderer

`src/service.rs` carried four async wrappers (`current_view`, `rescan`, `mark`, `unmark`), two render helpers (`render_view`, `render_section_from_raw`), and four types (`FlaggedView`, `RootSection`, `DomainError`, `MarkOutcome`). Its stated purpose was a web-agnostic service layer shared by the HTML UI and a future JSON API. After ADR-0027 consolidated the scan substrate and the marker IO behind `RawViewStore`, each wrapper collapsed to a three-line shape: take a result off the store, render it for the view mode, optionally wrap it in `Arc`. The only consumer of the four async wrappers was `src/web.rs`. Candidate 1 of the 2026-06 architecture review flagged the module as shallow: a public interface (four async fns plus four types) nearly as wide as the production implementation under it.

The four wrappers now inline into their four handlers in `src/web.rs`. `FlaggedView` and `RootSection` move to `src/web/render.rs` next to the markup that consumes them. The raw-to-packaged helpers move there too under terser names (`package_view` and `package_section`) so they do not collide with the existing markup-producing `render_view`. `DomainError` moves to `src/state.rs` next to the store that constructs it inside `write_marker` and `delete_marker`. `MarkOutcome` dissolves; the `web::mark` handler reads `Applied.created` directly off the store result. The `Arc<FlaggedView>` wrappers vanish; handlers hold `FlaggedView` by value, borrow one `&RootSection` for the section-shaped responses, and drop the view at the end of the response.

Alternatives we set aside. Growing `service.rs` to own the `ViewMode -> RawView -> FlaggedView -> Markup` pipeline (the other direction from the findings doc) would have crossed ADR-0027's "revisit if a future API surface needs raw scan output" tripwire without that API actually being on the horizon. Keeping the wrappers as thin pass-throughs would have kept a module hop per request, an extra `Arc::new` per response, and a misplaced home for `DomainError` without paying for any of it.

Revisit if a second HTTP-shaped consumer (JSON API, CLI HTTP harness) appears. At that point the shared response shape (package the raw view, render the section, attach `HX-*` headers and triggers) would be worth lifting back out, into a `response` module that owns the response packaging rather than a generic "service" layer.

## Related

- ADR-0002: marker writes edit cache in place. Preserved; the invariant still lives inside `RawViewStore::write_mark`.
- ADR-0022: cache holds raw scan output. Preserved; the store still holds raw.
- ADR-0027: substrate consolidated behind `RawViewStore`. This ADR extends the consolidation by removing the thin layer above the store.
