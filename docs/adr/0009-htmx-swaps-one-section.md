# A marker write swaps one section with vendored htmx, not a full reload

Superseded in part by ADR-0018 (commit `236073e`): the implicit progressive-enhancement framing here is gone, the UI requires JavaScript. The section-swap mechanism and the vendored-htmx decision below still stand, and ADR-0024 builds on the swap unit established here.

After a marker write succeeds, the page replaces only the affected root's `<section>`. The two buttons on each node row POST to `/mark` and target the closest `section.root`, swapping the server's response in as the new section (`hx-target="closest section.root"`, `hx-swap="outerHTML"`). The handler renders just that one section, which `service::mark` has already updated in memory, so the swap costs nothing past rendering a single root.

ADR-0018 later moved rescan onto the same htmx POST path as every other button (no full-page reload). A marker write is the case this ADR scopes: it changes one folder in one root and leaves the rest alone. Reloading the whole page would re-render every root and lose the open or closed state of the `<details>` elements the user has expanded. Swapping one section keeps that state and keeps the click responsive on a library with many roots.

We drive the swap with htmx rather than a client framework. The server already renders HTML with Maud, and htmx lets that markup ask for updates through attributes, with no build step and no client-side model to keep in sync. The runtime is one small script.

The script is vendored and served from `/static/htmx.min.js` rather than loaded from a CDN. The tool is self-hosted and often runs on loopback or behind a private tunnel, where outbound internet is not a given; a CDN reference would leave the page dead in those setups and add a third-party request besides. The file is pinned to htmx 2.0.4 and embedded with `include_str!`, so the binary carries its own copy and the version cannot move under the app.

Two details follow from putting buttons on rows that double as `<details>` summaries. The buttons are `type="button"` so they never submit their wrapping form on their own, and they call `event.stopPropagation()` so a click writes the marker without also toggling the summary it sits in. A failed write reuses the same swap: the handler returns the section with an inline error in place of the tree, so the partial-swap path carries failures as well as successes.
