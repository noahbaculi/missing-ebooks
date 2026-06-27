# The gap / needs-attention color is amber

Date: 2026-06-19.

## Context

We had moved this role to teal, on the reasoning that no amber both reads on a light surface and stays out of the mud. The accent picker reopened the question. It lets the reader recolor the gap role, and to keep a custom pick legible it derives the ink per theme: scan the base hue for the most vivid shade that clears AA against the tinted pill, dark for light mode and light for dark. That derivation runs for a custom base only; the default opts out and keeps the raw amber. Teal, rust, and magenta stay on as one-click presets.

## Decision

The color that marks a folder needing an ebook is amber. It carries the gaps-to-fill count, the "needs ebook" badge, the flagged folder icon, and the per-root gap chips. The base is `#f5a524` on both themes, the fill the soft badges tint from. The default ink is that same `#f5a524` on both themes, so the role is a single amber. The token keeps the name `--color-warning`, and the role is "this needs attention".

## Consequences

The default trades contrast for the amber look. As ink, `#f5a524` does not read well: on a white surface it is about 2:1, and on the pale tinted pill no better, below both AA and the 3:1 large-text floor. We take that on the default so the badge label, flagged icon, hero count, and collapsed dot stay the plain amber instead of a darker, muddier shade.

Amber sits clear of the rest of the palette: indigo on the primary, green on success, red on error. It is warm but more yellow than the error red, so a gap badge and an error badge do not read as the same alarm, and it stays plainly distinct from the cool success green beside it in the summary chips. A custom pick sets `--color-warning` and the derived `--color-warning-text` inline on `<html>`; the pre-paint bootstrap applies a saved pick before first paint so it never flashes the default ink.
