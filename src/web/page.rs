//! The page shell and the navbar/popover chrome. Imported by `web::render`,
//! never the reverse: see the implementation plan at
//! `docs/superpowers/plans/2026-06-23-b-render-split-and-assets-triage.md`
//! for the call-graph rationale. None of these helpers touch domain types
//! like `Node` or `FlaggedView`; the shell takes the body as a `Markup`
//! parameter so the chrome and the tree never share a module.

use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::tree::ViewMode;

/// The favicon as an inline SVG data URI, so the tab gets an identity and the
/// browser stops requesting `/favicon.ico`. The "book wearing headphones" glyph,
/// no backdrop. It draws in `currentColor`; an embedded `<style>` binds that to
/// indigo `%23605dff` on light tab strips and a lighter `%23c7c5ff` on dark ones
/// via `prefers-color-scheme`. WARN: Chrome, Firefox, and Edge honor the media
/// query inside a favicon, Safari ignores it and shows the light-mode indigo
/// throughout. Source art lives at `assets/brand/favicon.svg`. This constant is
/// the percent-encoded data-URI form that goes into `<link rel="icon">`. Keep
/// the two in sync if the mark changes. The navbar's inline copy reads
/// `assets/svg/brand.svg`, which itself derives from `assets/brand/favicon.svg`.
/// Spec B chose to keep the data URI hand-written rather than reconstruct it
/// with `const_format::concatcp!`: the `%`-escapes are not byte-identical to a
/// plain SVG file, so a build-time concat would not round-trip.
const FAVICON_HREF: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-linecap='round' stroke-linejoin='round'%3E%3Cstyle%3Esvg{color:%23605dff}@media(prefers-color-scheme:dark){svg{color:%23c7c5ff}}%3C/style%3E%3Cpath d='M4.5 14v-2a7.5 7.5 0 0 1 15 0v2' stroke-width='2'/%3E%3Crect x='3' y='13' width='3.2' height='6' rx='1.6' fill='currentColor' stroke='none'/%3E%3Crect x='17.8' y='13' width='3.2' height='6' rx='1.6' fill='currentColor' stroke='none'/%3E%3Cpath d='M12 11.8c-1.2-.85-3-.85-4.2 0v4.8c1.2-.85 3-.85 4.2 0c1.2-.85 3-.85 4.2 0v-4.8c-1.2-.85-3-.85-4.2 0z' stroke-width='1.4'/%3E%3Cpath d='M12 11.8v4.8' stroke-width='1.2'/%3E%3C/svg%3E";

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
/// keyboard shortcuts reference. The native popover API drives open/close. Behavior
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
            popovertarget="settings-panel" { (PreEscaped(include_str!("../../assets/svg/cog.svg"))) }
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
/// its navbar slot and never reflows in. The input renders `disabled`. `app.js`
/// clears that on `DOMContentLoaded` once the tree and handler are wired, so during
/// load the box reads greyed-but-present, not a dead box accepting input before the
/// filter works. The `/` key focuses it and Escape clears it, both in `app.js`. A
/// themed clear button sits after the input, hidden until the box holds text.
pub(super) fn search_box() -> Markup {
    html! {
        div.search id="search" {
            (PreEscaped(include_str!("../../assets/svg/search.svg")))
            input.search-input id="search-input" type="search"
                placeholder="Filter folders" aria-label="Filter folders"
                autocomplete="off" disabled;
            button.search-clear id="search-clear" type="button"
                aria-label="Clear filter" hidden {
                (PreEscaped(include_str!("../../assets/svg/clear.svg")))
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
/// label, and matching glyph from the firing button, then opens it. A marker write
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
                    span.confirm-icon data-confirm-icon=".no_ebook" { (PreEscaped(include_str!("../../assets/svg/no-entry.svg"))) }
                    span.confirm-icon data-confirm-icon=".ebook_elsewhere" hidden { (PreEscaped(include_str!("../../assets/svg/ebook-elsewhere.svg"))) }
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
/// in data attributes so it is defined and tested here in one place. The client
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
                span.toast-icon.toast-icon-success { (PreEscaped(include_str!("../../assets/svg/check.svg"))) }
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
                // Pre-paint bootstrap; see assets/prepaint.js. The
                // `deriveWarningInk` helper inside is the parity copy of the
                // one in assets/app.js, checked by tests/accent/derive.test.mjs.
                script { (PreEscaped(include_str!("../../assets/prepaint.js"))) }
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
                    h1 { a href="/" { (PreEscaped(include_str!("../../assets/svg/brand.svg"))) "Missing Ebooks" } }
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
