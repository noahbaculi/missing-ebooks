//! The page shell and the navbar/popover chrome. Imported by `web::render`,
//! never the reverse: see the implementation plan at
//! `docs/superpowers/plans/2026-06-23-b-render-split-and-assets-triage.md`
//! for the call-graph rationale. None of these helpers touch domain types
//! like `Node` or `FlaggedView`; the shell takes the body as a `Markup`
//! parameter so the chrome and the tree never share a module.

use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::service::ViewMode;

/// Pre-paint bootstrap: resolves the saved theme (or the OS preference for
/// "system" or an unset value), sets `data-theme` on <html> before first paint so
/// there is no flash, applies the two depth-typography opt-outs, and applies a
/// saved custom accent inline. The default accent writes no override, so the
/// stylesheet's tuned tokens apply. `deriveWarningInk` mirrors the copy in
/// `app.js`, fenced by the `ACCENT-DERIVE` markers. `tests/accent/derive.test.mjs`
/// checks the two agree and that the ink clears AA. Interactive controls live in
/// `app.js`.
const PREPAINT_JS: &str = r#"(function () {
  var saved = localStorage.getItem('theme');
  var t = (saved === 'light' || saved === 'dark')
    ? saved
    : (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  document.documentElement.dataset.theme = t;
  if (localStorage.getItem('boldTopFolder') === 'off') {
    document.documentElement.dataset.boldTop = 'off';
  }
  if (localStorage.getItem('italicNestedFolders') === 'off') {
    document.documentElement.dataset.italicNested = 'off';
  }

  // ACCENT-DERIVE:BEGIN. Mirrored in assets/app.js, parity checked by tests/accent/derive.test.mjs.
  function luminance(hex) {
    var ch = [1, 3, 5].map(function (i) {
      var c = parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
  }
  function contrastRatio(a, b) {
    var l1 = luminance(a), l2 = luminance(b);
    var hi = Math.max(l1, l2), lo = Math.min(l1, l2);
    return (hi + 0.05) / (lo + 0.05);
  }
  function mixColors(hex, pct, surf) {
    var f = pct / 100, out = '#';
    for (var i = 1; i < 6; i += 2) {
      var h = parseInt(hex.slice(i, i + 2), 16);
      var s = parseInt(surf.slice(i, i + 2), 16);
      out += Math.round(h * f + s * (1 - f)).toString(16).padStart(2, '0');
    }
    return out;
  }
  function hexToHsl(hex) {
    var r = parseInt(hex.slice(1, 3), 16) / 255;
    var g = parseInt(hex.slice(3, 5), 16) / 255;
    var b = parseInt(hex.slice(5, 7), 16) / 255;
    var mx = Math.max(r, g, b), mn = Math.min(r, g, b), l = (mx + mn) / 2, h = 0, s = 0;
    if (mx !== mn) {
      var d = mx - mn;
      s = l > 0.5 ? d / (2 - mx - mn) : d / (mx + mn);
      if (mx === r) h = (g - b) / d + (g < b ? 6 : 0);
      else if (mx === g) h = (b - r) / d + 2;
      else h = (r - g) / d + 4;
      h /= 6;
    }
    return { h: h * 360, s: s * 100, l: l * 100 };
  }
  function hslToHex(h, s, l) {
    h /= 360; s /= 100; l /= 100;
    function hue2(p, q, t) {
      if (t < 0) t += 1;
      if (t > 1) t -= 1;
      if (t < 1 / 6) return p + (q - p) * 6 * t;
      if (t < 1 / 2) return q;
      if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
      return p;
    }
    var r = l, g = l, b = l;
    if (s !== 0) {
      var q = l < 0.5 ? l * (1 + s) : l + s - l * s, p = 2 * l - q;
      r = hue2(p, q, h + 1 / 3); g = hue2(p, q, h); b = hue2(p, q, h - 1 / 3);
    }
    return '#' + [r, g, b].map(function (v) {
      return Math.round(v * 255).toString(16).padStart(2, '0');
    }).join('');
  }
  function deriveWarningInk(base, theme) {
    var surf = theme === 'dark' ? '#1d232a' : '#ffffff';
    var bg = mixColors(base, 16, surf);
    var hsl = hexToHsl(base);
    var sat = Math.max(hsl.s, 42);
    var strong = [], ok = [];
    for (var L = 8; L <= 94; L++) {
      var c = hslToHex(hsl.h, sat, L), r = contrastRatio(c, bg);
      if (r >= 5.5) strong.push({ c: c, l: L });
      else if (r >= 4.5) ok.push({ c: c, l: L });
    }
    var pool = strong.length ? strong : ok;
    if (pool.length) {
      var best = pool[0];
      for (var i = 1; i < pool.length; i++) {
        var better = theme === 'dark' ? pool[i].l < best.l : pool[i].l > best.l;
        if (better) best = pool[i];
      }
      return best.c;
    }
    return hslToHex(hsl.h, sat, theme === 'dark' ? 90 : 15);
  }
  // ACCENT-DERIVE:END

  var accent = localStorage.getItem('accent');
  if (/^#[0-9a-fA-F]{6}$/.test(accent || '') && accent.toLowerCase() !== '#f5a524') {
    document.documentElement.style.setProperty('--color-warning', accent);
    document.documentElement.style.setProperty('--color-warning-text', deriveWarningInk(accent, t));
  }
})();"#;

/// The favicon as an inline SVG data URI, so the tab gets an identity and the
/// browser stops requesting `/favicon.ico`. The "book wearing headphones" glyph,
/// no backdrop. It draws in `currentColor`; an embedded `<style>` binds that to
/// indigo `%23605dff` on light tab strips and a lighter `%23c7c5ff` on dark ones
/// via `prefers-color-scheme`. WARN: Chrome, Firefox, and Edge honor the media
/// query inside a favicon, Safari ignores it and shows the light-mode indigo
/// throughout. Source art lives at `assets/brand/favicon.svg`; keep it and
/// `BRAND_SVG` in sync if the mark changes.
const FAVICON_HREF: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-linecap='round' stroke-linejoin='round'%3E%3Cstyle%3Esvg{color:%23605dff}@media(prefers-color-scheme:dark){svg{color:%23c7c5ff}}%3C/style%3E%3Cpath d='M4.5 14v-2a7.5 7.5 0 0 1 15 0v2' stroke-width='2'/%3E%3Crect x='3' y='13' width='3.2' height='6' rx='1.6' fill='currentColor' stroke='none'/%3E%3Crect x='17.8' y='13' width='3.2' height='6' rx='1.6' fill='currentColor' stroke='none'/%3E%3Cpath d='M12 11.8c-1.2-.85-3-.85-4.2 0v4.8c1.2-.85 3-.85 4.2 0c1.2-.85 3-.85 4.2 0v-4.8c-1.2-.85-3-.85-4.2 0z' stroke-width='1.4'/%3E%3Cpath d='M12 11.8v4.8' stroke-width='1.2'/%3E%3C/svg%3E";

/// The brand mark drawn inline at the head of the navbar, the same "book wearing
/// headphones" glyph as the favicon. It draws in `currentColor`, which the navbar
/// binds to the primary indigo so the mark follows the theme, and is `aria-hidden`
/// since the `<h1>` already names the app. Same art as `FAVICON_HREF` and
/// `assets/brand/favicon.svg`. Keep the three in sync if the mark changes.
const BRAND_SVG: &str = r##"<svg class="brand-mark" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4.5 14v-2a7.5 7.5 0 0 1 15 0v2" stroke-width="2"/><rect x="3" y="13" width="3.2" height="6" rx="1.6" fill="currentColor" stroke="none"/><rect x="17.8" y="13" width="3.2" height="6" rx="1.6" fill="currentColor" stroke="none"/><path d="M12 11.8c-1.2-.85-3-.85-4.2 0v4.8c1.2-.85 3-.85 4.2 0c1.2-.85 3-.85 4.2 0v-4.8c-1.2-.85-3-.85-4.2 0z" stroke-width="1.4"/><path d="M12 11.8v4.8" stroke-width="1.2"/></svg>"##;

/// Gear glyph for the settings menu trigger. Inherits `currentColor`.
const COG_SVG: &str = r##"<svg class="icon" aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"##;

/// Magnifying glass for the search-links dropdown trigger. Inherits `currentColor`.
const SEARCH_SVG: &str = r##"<svg class="icon" aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.35-4.35"/></svg>"##;

/// Check mark for the "no gaps in this root" state. Inherits `currentColor`.
const CHECK_SVG: &str = r##"<svg class="icon" aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M5 13l4 4L19 7"/></svg>"##;

/// A thin × for the filter's clear button: two diagonal strokes, no circle, in
/// `currentColor` so it follows the button's muted-to-base hover color.
const CLEAR_SVG: &str = r##"<svg aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>"##;

/// A "no entry" sign (circle with a horizontal bar) for the sheet's "No ebook"
/// row. Shown only inside the mobile sheet. Inherits `currentColor`.
const NO_ENTRY_SVG: &str = r##"<svg class="icon" aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M7 12h10"/></svg>"##;

/// A book with a small check, marking that this audiobook's ebook is accounted
/// for somewhere else rather than missing. Shown on the sheet's "Ebook
/// elsewhere" button. Inherits `currentColor`.
const EBOOK_ELSEWHERE_SVG: &str = r##"<svg class="icon" aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H19a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1H6.5a1 1 0 0 1 0-5H20"/><path d="m9 9.5 2 2 4-4"/></svg>"##;

/// The gaps-only / show-all view control for the navbar. The segment for the
/// current view is inert and marked `aria-current`, the other is a GET link to its
/// view. Switching reshapes every root, so it is a full-page navigation, and the
/// choice is not persisted.
pub(super) fn view_toggle(mode: ViewMode) -> Markup {
    html! {
        div.segmented role="group" aria-label="View" {
            @match mode {
                ViewMode::GapsOnly => {
                    span.segment.segment-active aria-current="page" { "Gaps only" }
                    a.segment href="/?view=all" { "All folders" }
                }
                ViewMode::All => {
                    a.segment href="/" { "Gaps only" }
                    span.segment.segment-active aria-current="page" { "All folders" }
                }
            }
        }
    }
}

/// The navbar settings control: a cog opening a popover with the theme choice, the
/// confirm-before-marking toggle, the folder-depth switches, and a read-only
/// keyboard shortcuts reference. The native popover API drives open/close; behavior
/// lives in `app.js`, which also opens the panel on `?`. Segments and switches
/// render in their default state (System, all on), reconciled against localStorage
/// once `app.js` runs. The shortcuts section hides on mobile, where there is no
/// keyboard.
pub(super) fn settings_menu() -> Markup {
    html! {
        button.btn.btn-ghost.btn-square.settings-cog type="button"
            aria-label="Settings"
            title="Settings"
            aria-haspopup="menu"
            popovertarget="settings-panel" { (PreEscaped(COG_SVG)) }
        div.settings-panel id="settings-panel" popover="auto" aria-label="Settings" {
            div.settings-head { "Theme" }
            div.settings-row.settings-row-theme {
                div.segmented role="group" aria-label="Theme" {
                    button.segment type="button" data-theme-choice="light" { "Light" }
                    button.segment type="button" data-theme-choice="dark" { "Dark" }
                    button.segment.segment-active type="button"
                        data-theme-choice="system" aria-current="true" { "System" }
                }
            }
            div.settings-row {
                span.settings-label { "Accent Color" }
                span.accent-ctl {
                    span.accent-dots {
                        button.accent-dot type="button" data-accent="#06b6d4"
                            style="background:#06b6d4"
                            aria-label="Teal" title="Teal" {}
                        button.accent-dot type="button" data-accent="#c2410c"
                            style="background:#c2410c"
                            aria-label="Rust" title="Rust" {}
                        button.accent-dot type="button" data-accent="#a21caf"
                            style="background:#a21caf"
                            aria-label="Magenta" title="Magenta" {}
                    }
                    input.accent-swatch id="accent-input" type="color"
                        value="#f5a524" aria-label="Accent color";
                }
            }
            div.settings-head { "Settings" }
            div.settings-row {
                span.settings-label {
                    "Confirm before marking"
                    span.settings-sub { "Ask before writing a marker" }
                }
                label.switch {
                    input id="confirm-toggle" type="checkbox" checked;
                    span.switch-track {}
                }
            }
            div.settings-head { "Folder depth styling" }
            div.settings-row {
                span.settings-label { "Bold top folder" }
                label.switch {
                    input id="bold-top-toggle" type="checkbox" checked;
                    span.switch-track {}
                }
            }
            div.settings-row {
                span.settings-label { "Italicize nested folders" }
                label.switch {
                    input id="italic-nested-toggle" type="checkbox" checked;
                    span.switch-track {}
                }
            }
            div.settings-shortcuts {
                div.settings-head { "Keyboard shortcuts" }
                dl.settings-shortcuts-list {
                    dt { kbd { "j" } " / " kbd { "k" } } dd { "Move between gaps" }
                    dt { kbd { "r" } } dd { "Rescan the library" }
                    dt { kbd { "/" } } dd { "Focus the filter" }
                    dt { kbd { "Enter" } } dd { "Exit the filter" }
                    dt { kbd { "?" } } dd { "Show this list" }
                    dt { kbd { "Esc" } } dd { "Clear the filter or selection" }
                }
            }
        }
    }
}

/// The navbar filter input. The box renders visible from first paint so it holds
/// its navbar slot and never reflows in. The input renders `disabled`; `app.js`
/// clears that on `DOMContentLoaded` once the tree and handler are wired, so during
/// load the box reads greyed-but-present, not a dead box accepting input before the
/// filter works. The `/` key focuses it and Escape clears it, both in `app.js`. A
/// themed clear button sits after the input, hidden until the box holds text.
pub(super) fn search_box() -> Markup {
    html! {
        div.search id="search" {
            (PreEscaped(SEARCH_SVG))
            input.search-input id="search-input" type="search"
                placeholder="Filter folders" aria-label="Filter folders"
                autocomplete="off" disabled;
            button.search-clear id="search-clear" type="button"
                aria-label="Clear filter" hidden {
                (PreEscaped(CLEAR_SVG))
            }
        }
    }
}

/// The "no matches" line for an active filter that matches nothing. Hidden until
/// `app.js` shows it, and a polite live region so the empty result is announced. It
/// sits beside `#roots` so a rescan swap leaves it in place.
pub(super) fn search_empty() -> Markup {
    html! {
        p.search-empty id="search-empty" role="status" aria-live="polite" hidden {
            "No folders match your filter."
        }
    }
}

/// The marker-write confirmation, rendered once at the page level so it survives
/// the htmx section swaps. `app.js` fills the title, folder, file chip, confirm
/// label, and matching glyph from the firing button, then opens it; a marker write
/// fires only through it.
pub(super) fn confirm_dialog() -> Markup {
    html! {
        dialog.confirm-dialog id="confirm-mark" aria-labelledby="confirm-title" {
            h2.confirm-title id="confirm-title" { "Mark this folder?" }
            p.confirm-body {
                "Writes a "
                span.confirm-chip id="confirm-file" { ".no_ebook" }
                " file to "
                strong id="confirm-folder" { "this folder" }
                ", covering this folder and everything beneath it."
            }
            label.confirm-again-label {
                input id="confirm-again" type="checkbox";
                "Don't ask again on this device"
            }
            div.confirm-actions {
                button.btn.btn-outline id="confirm-cancel" type="button" { "Cancel" }
                button.btn.btn-primary id="confirm-accept" type="button" {
                    span.confirm-icon data-confirm-icon=".no_ebook" { (PreEscaped(NO_ENTRY_SVG)) }
                    span.confirm-icon data-confirm-icon=".ebook_elsewhere" hidden { (PreEscaped(EBOOK_ELSEWHERE_SVG)) }
                    span id="confirm-accept-label" { "Confirm" }
                }
            }
        }
    }
}

/// The rescan progress bar. A slim indeterminate bar pinned above `#roots` while an
/// in-place rescan runs: it is the `hx-indicator`, so htmx adds `htmx-request` to it
/// for the request's duration and the stylesheet slides the bar and dims the tree in
/// place. Both sit behind a CSS show-delay, so a fast scan shows nothing. Hidden
/// otherwise.
pub(super) fn scan_bar() -> Markup {
    html! {
        div.scan-bar id="scan-bar" aria-hidden="true" {}
    }
}

/// The connection-status banner: a polite live region pinned to the top of the
/// page, hidden until `app.js` reveals it and sets a state class. State copy lives
/// in data attributes so it is defined and tested here in one place; the client
/// only chooses which message to show.
pub(super) fn conn_banner() -> Markup {
    html! {
        div.conn-banner id="conn-banner" role="status" aria-live="polite" hidden
            data-msg-offline="You're offline. Changes can't be saved."
            data-msg-retrying="Lost connection. Retrying…"
            data-msg-failed="Couldn't reach the server. Your change wasn't saved."
            data-msg-failed-rescan="Couldn't reach the server. The library wasn't rescanned."
            data-msg-reconnected="Reconnected." {
            span.conn-banner-spinner aria-hidden="true" {}
            span.conn-banner-msg {}
        }
    }
}

/// The page-level toast machinery, rendered once so it survives the htmx section
/// swaps: an empty stack container plus a template. `app.js` clones the template
/// per successful mark, fills it with the undo offer, and appends it to the stack,
/// keeping at most three. Write failures stay inline by the row, not here.
pub(super) fn toast() -> Markup {
    html! {
        div.toast-stack id="toast-stack" {}
        template id="toast-template" {
            div.toast {
                span.toast-icon.toast-icon-success { (PreEscaped(CHECK_SVG)) }
                div.toast-msg {}
                button.btn.btn-outline.btn-xs.toast-undo type="button" { "Undo" }
                button.toast-close type="button" aria-label="Dismiss" { "\u{00D7}" }
            }
        }
    }
}

/// The HTML document shell: head, noscript notice, connection banner, SSE
/// listener, navbar, confirm dialog, toast machinery, and the script tags.
/// Wraps a prebuilt `body` markup (typically the gap summary plus the
/// `#roots` block, assembled by `render::render_view`). Pure shell: takes
/// no domain types, only the current `ViewMode` for the navbar and the SSE
/// query string.
pub(crate) fn page(mode: ViewMode, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Missing Ebooks" }
                link rel="icon" href=(FAVICON_HREF);
                script { (PreEscaped(PREPAINT_JS)) }
                link rel="stylesheet" href="/static/app.css";
            }
            body {
                noscript {
                    div.noscript-notice {
                        "Missing Ebooks needs JavaScript to run. Please enable it and reload."
                    }
                }
                (conn_banner())
                // The autosync SSE listener: opens a /events connection on load
                // and routes each section event's OOB-swap payload to its target
                // `<section id="root-N-section">` by ID (see ADR-0023, ADR-0024).
                div hx-ext="sse"
                    sse-connect=(format!("/events?view={}", mode.as_query()))
                    sse-swap="section,snapshot" {}
                nav.navbar {
                    // The title is a home link: a plain GET to "/" that survives the
                    // htmx swaps, landing on the default gaps-only view with no filter,
                    // the conventional reset for a wordmark.
                    h1 { a href="/" { (PreEscaped(BRAND_SVG)) "Missing Ebooks" } }
                    // The spacer sits right after the title so the title alone pins
                    // left and everything else groups on the right. Mobile reorders
                    // every control by flex `order`, so this DOM move is desktop-only.
                    span.spacer {}
                    (search_box())
                    (view_toggle(mode))
                    (settings_menu())
                    form hx-target="#roots" hx-swap="innerHTML"
                        hx-indicator="#scan-bar, #rescan-btn"
                        hx-disabled-elt="#rescan-btn" {
                        input type="hidden" name="view" value=(mode.as_query());
                        // The button posts via htmx (like the marker buttons) and sits in
                        // the nav, outside the swapped `#roots`, so htmx keeps it across
                        // the request. `hx-disabled-elt` locks it so a second click cannot
                        // double-scan, and the disabled state dims it (`app.css`). The
                        // label stays put, and both clear once the swap settles.
                        button.btn.btn-primary id="rescan-btn" type="button"
                            hx-post="/rescan" hx-include="closest form" { "Rescan" }
                    }
                }
                (body)
                (confirm_dialog())
                (toast())
                script src="/static/htmx.min.js" {}
                script src="/static/htmx-sse.js" {}
                script src="/static/app.js" {}
            }
        }
    }
}
