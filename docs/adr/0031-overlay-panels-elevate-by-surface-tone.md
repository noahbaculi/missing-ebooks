# Overlay panels elevate by surface tone, not by scrim

Date: 2026-07-03.

## Context

The settings panel and the marker-write confirm dialog both open as centered popovers with a dimmed backdrop. In light mode the pattern reads correctly: the panel is `#ffffff` on a `#f2f3f5` page, so ~98 luma units of tone gap plus a 1px border plus a soft drop shadow all sell "elevated card," and a 35% black scrim knocks the light page down enough that the modal reads as modal. In dark, none of those cues carried. The panel was `#1d232a` and the page was `#191e24` (~13 luma units apart), the drop shadow is invisible on dark-on-dark, and the same 35% scrim moves a near-black page almost nothing. The result was a panel that shared a tone with the page and floated on nothing.

Blur on the backdrop (`backdrop-filter: blur(...)`) fixes the perception cheaply, but it hides the page. That matters here specifically because one of the panel's own controls is the accent picker: users change the accent to preview it against the live page, so anything that obscures the page defeats the control it lives beside.

## Decision

Overlay panels separate from the page primarily by *surface elevation*, not by scrim opacity or backdrop blur. A new token, `--color-surface-raised`, is the fill for anything that sits on top of the page: the settings panel, the confirm dialog, and the mobile bottom sheets. Its value is `#ffffff` in light (flush with existing panels) and `#2b3139` in dark (a step lighter than the `#191e24` body). The token is orthogonal to the `--color-base-100/200/300` scale, which continues to describe the page and its cards; elevated surfaces are their own role.

Backdrops stay a soft 45% black scrim across all three surfaces. No `backdrop-filter`, no per-theme scrim tuning, no `prefers-reduced-transparency` branch. The scrim is deliberately light so the page remains legible while a panel is open, which the accent picker's live preview depends on. Elevation via surface tone is what does the work; the scrim only signals "the page is in the background right now."

## Consequences

The dark panel now reads as elevated without leaning on transparency or motion, so the `prefers-reduced-transparency` preference needs no override, and there is no `backdrop-filter` cost on low-end hardware or older iOS to worry about. Users retain full visibility of the page while adjusting settings, so the accent picker's live retint of icons, badges, and the wordmark is visible without closing the panel.

The 1px panel border and drop shadow are now visually redundant in dark: tone contrast does the separation work by itself. They stay because they contribute in light, where the shadow reads clearly, and because removing them would give the panel a slightly floaty edge on high-DPI displays. The confirm dialog carries a stronger `0 16px 48px rgb(0 0 0 / 28%)` shadow than the settings panel; in dark this remains near-invisible, and that is fine since the raised surface does the separation.

Introducing `--color-surface-raised` as a role distinct from `--color-base-100` means the accent-derive test's `SURFACE = { light: "#ffffff", dark: "#1d232a" }` still describes the correct backdrop for the amber warning pill, since that pill still tints into `--color-base-100`. If a future component wants the raised look for a pill or badge, it should target `--color-surface-raised` and the derivation would need a matching entry.

## Related

Amends nothing directly but shares the surface-token vocabulary with [ADR-0021](0021-gap-attention-color-is-amber.md), which introduced per-theme token derivation for the accent picker.
