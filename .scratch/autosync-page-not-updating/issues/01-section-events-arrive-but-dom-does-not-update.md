# Section events arrive in the browser but the page DOM does not update

Status: ready-for-human
Resolved: see `## Comments` below.

## Symptom

With autosync running and a tab open to the index page, a filesystem change under one of the library roots does not visibly update the page until the user clicks Rescan. The new flagged folder appears only after that explicit click, even though the autosync loop has emitted SSE `section` events the browser received.

Confirmed in Chrome against the production `web::router` (via `examples/explore mixed-forest --port 8919`) on a freshly hard-reloaded tab (Cmd+Shift+R).

## What is verified working

- **Scanner**: a `GET /?view=gaps` rendered fresh after the touch shows the new flagged folder, so the warm scan and dir-index pick up the change.
- **Autosync loop**: tracing logs show steady warm scans on the configured interval; no exits or panics.
- **SSE push pipeline**: a parallel `curl -N http://127.0.0.1:8919/events?view=gaps` connection catches both the snapshot event and subsequent `event: section` payloads carrying the changed root's OOB-wrapped HTML within roughly one autosync tick of the touch.
- **Browser receives the events**: in Chrome DevTools → Network → `events?view=gaps` → EventStream tab, `event: section` rows appear with the correct OOB-wrapped HTML for the changed root (snapshot first, then one `section` event per real filesystem change). The user pasted three such rows showing the new folder names propagating live.

So the autosync backend is healthy end-to-end up to and including the browser's EventSource. The break is downstream of that: the HTMX-SSE swap that should consume the event and apply the OOB swap to `#root-N-section` is not taking effect.

## What is unaffected

The seven commits from `docs/superpowers/plans/2026-06-21-autosync-review-fixes.md` (currently on `main` ahead of `origin/main`) are not implicated. None of them touch the HTMX-SSE wiring on the page, the `assets::htmx_sse_script`, or any client-side JS. The OOB consolidation (Task 4) changed the *renderer* but the existing `tests/cache_render_byte_equal.rs` and the new `render_oob_section_bytes_match_a_direct_single_oob_section_render` test in `src/autosync.rs` both assert the bytes are unchanged. The curl-level verification above confirms the wire format is correct.

## Page wiring

The listening element in the rendered page reads:

```html
<div hx-ext="sse" sse-connect="/events?view=gaps" sse-swap="section,snapshot">
```

The wrapped section payload sent on each `section` event is the OOB-marked div produced by `web::render::single_oob_section`:

```html
<div hx-swap-oob="outerHTML:#root-0-section transition:true">
  <section class="card root" id="root-0-section" data-root="0">
    …
  </section>
</div>
```

The vendored extension is htmx-sse 2.2.4 (`assets::htmx_sse_script`, served from `/static`). The base htmx is `htmx.min.js` in the same directory.

## Hypothesis

The likeliest cause is that htmx-sse 2.2.4's swap path for named events does not run the full htmx swap pipeline that processes `hx-swap-oob` on incoming content. The snapshot event happens to look correct on first page load because the index page render already produced identical `#root-N-section` markup before any swap ran. Section events, however, would only visibly change the page if the OOB processing actually fires; if the extension just sets the listening div's innerHTML and stops, the OOB attribute is inert and the existing `#root-0-section` sibling is untouched.

This is a hypothesis. Confirming it needs a console probe (`htmx.logAll()` in the page or a small breakpoint in the htmx-sse extension's swap handler) to observe whether `oobSwap` is reached on each `section` event.

## Reproduction steps

1. `cargo run --example explore -- mixed-forest --port 8919`
2. Open `http://127.0.0.1:8919/` in Chrome.
3. Open DevTools → Network → click the `events?view=gaps` row → EventStream tab.
4. In another terminal: `mkdir -p /private/tmp/explore-*/mixed-forest/Library/NewFolder/Book && touch /private/tmp/explore-*/mixed-forest/Library/NewFolder/Book/01.mp3` (use the real tempdir path printed by the harness).
5. Within ~10 s, observe an `event: section` row appear in the EventStream with `NewFolder` in the HTML, but the page itself shows no new entry under Library.
6. Click Rescan: the page now shows `NewFolder`.

## Suggested next steps

- Add `htmx.logAll()` in DevTools console and observe whether HTMX logs an `oobSwap` event when a `section` event arrives. If it does not, the swap is stopping before OOB processing.
- Check whether htmx-sse 2.2.4's swap respects `hx-swap` on the listening element. Setting `hx-swap="none"` on the listening div and relying entirely on OOB may force the extension through the right path; alternatively, marking the inner data with `hx-swap-oob="true"` alone (rather than `outerHTML:#…`) is the documented HTMX OOB pattern and may be the one htmx-sse routes correctly.
- Failing that, consider replacing the htmx-sse extension with a small inline EventSource handler that calls `htmx.process(...)` on the received fragment so the full swap pipeline always runs.

## Scope

- Confirm the root cause via DevTools tracing.
- Fix the wiring so `section` events visibly update the open page within one autosync tick.
- Add a Playwright (or similar) regression test asserting that an SSE-pushed `section` event mutates the DOM, not just the EventStream buffer.

## Out of scope

- Any backend changes (the SSE pipeline is verified correct).
- Bumping htmx or htmx-sse to a newer major version. A bump may be the fix but should be its own ticket with its own ADR.

## Comments

Resolved. The hypothesis in the body (htmx-sse 2.2.4 skipping the OOB pipeline) was wrong. htmx-sse's `swap` in `assets/htmx-sse.js:283` calls `api.swap(...)`, which is the public `htmx.swap` (`$e` in `htmx.min.js`), which calls `ze(...)` to find every `[hx-swap-oob]` element and routes each one through `He(...)`. OOB processing was running on every section event.

The actual break was the OOB attribute value. htmx 2.0.4's `He` parses `hx-swap-oob` by splitting on the *first* colon: everything before is the swap style, everything after is the CSS selector. The payload from `src/web/render.rs::single_oob_section` was `hx-swap-oob="outerHTML:#root-N-section transition:true"`, so the selector parsed as `#root-N-section transition:true`, which is invalid CSS. `htmx:oobErrorNoTarget` fired silently, the OOB wrapper was discarded, the page never updated. The snapshot path *looked* fine on first load only because `render::page` had already produced identical `#root-N-section` markup before any SSE event could fail to swap it.

Fix in `src/web/render.rs:422`: drop `transition:true` from the attribute, leaving `outerHTML:#root-{root}-section`. Verified end-to-end on `examples/explore mixed-forest` with Playwright: two filesystem changes, two `htmx:oobAfterSwap` events, zero `htmx:oobErrorNoTarget`, new folders visible in the DOM.

Pinned by `src/web/render.rs::tests::single_oob_section_attribute_survives_htmx_first_colon_parse`, which asserts the OOB attribute parses as `outerHTML` plus a plain `#id` selector with no whitespace and no extra colons. ADR-0024 amended to record the parser constraint and to flag the view-transition route as a separate global decision.

`transition:true` was never delivering a fade. htmx 2.x's OOB parser has no per-element transition slot. Reinstating the section fade is `htmx.config.globalViewTransitions = true`, which would also apply to Rescan and marker swaps; tried it during diagnosis, the effect was imperceptible on this DOM, reverted. Filed as a follow-up if the section fade is wanted later.
