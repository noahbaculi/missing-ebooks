//! The page shell and the navbar/popover chrome. Imported by `web::render`,
//! never the reverse: see the implementation plan at
//! `docs/superpowers/plans/2026-06-23-b-render-split-and-assets-triage.md`
//! for the call-graph rationale. None of these helpers touch domain types
//! like `Node` or `FlaggedView`; the shell takes the body as a `Markup`
//! parameter so the chrome and the tree never share a module.

use maud::{Markup, PreEscaped, html};

use crate::service::ViewMode;

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
