# The web UI requires JavaScript

Date: 2026-06-12.

## Context

We had carried a partial no-JS fallback: the rescan form posted natively and the handler answered a non-htmx request with a 303 redirect (Post/Redirect/Get). Marker writes never had an equivalent, since their buttons were already `type="button"`. The fallback covered one action out of several and was reached only with scripting off, while comments across the code described a no-JS path that no longer worked.

## Decision

Every action the page offers runs through htmx. Marker writes and undo post from `type="button"` buttons with `hx-post`, and the rescan button does the same. None of them is a native form submit, so with scripting disabled the buttons do nothing. The page is rendered by Maud on the server, htmx and `app.js` are served from the binary, and the tool is self-hosted, so the script is always present in any real deployment.

We removed it: the rescan handler renders the sections for every request, the rescan button posts through htmx like the rest, and a `<noscript>` strip tells a scripting-disabled visitor to enable JavaScript and reload.

## Consequences

This leaves one render path for rescan instead of two, and one interaction model across every button. The demo binary keeps its own 303 redirects for rescan and reset, since a public, shared, crawlable deployment has its own reasons to keep Post/Redirect/Get. That is a separate decision from the self-hosted UI.

Supersedes the progressive-enhancement framing of ADR 0009. The rest of 0009 stands: the section-swap mechanism and the vendored-htmx decision are unchanged.
