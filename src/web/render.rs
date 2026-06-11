//! All Maud markup: the page shell, the per-root section, the per-node rows, and
//! the SVG/JS constants those use. Kept separate from the router so the markup is
//! the test surface and `web.rs` stays handlers and glue.

use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::config::SearchLink;
use crate::query::clean_query;
use crate::service::{FlaggedView, RootSection, RootState, ViewMode};
use crate::tree::Node;

/// Pre-paint theme bootstrap: resolves the saved choice, or the OS preference for
/// "system" / an unset value, and sets `data-theme` on <html> before first paint
/// so there is no flash. The interactive theme control lives in `app.js`.
const PREPAINT_THEME_JS: &str = r#"(function () {
  var saved = localStorage.getItem('theme');
  var t = (saved === 'light' || saved === 'dark')
    ? saved
    : (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  document.documentElement.dataset.theme = t;
})();"#;

/// Gear glyph for the settings menu trigger. Inherits `currentColor`.
const COG_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"##;

/// Caret that rotates open when its folder is expanded. Inherits `currentColor`.
const CHEVRON_SVG: &str = r##"<svg class="chev" viewBox="0 0 16 16" fill="currentColor"><path d="M6 4l4 4-4 4z"/></svg>"##;

/// Magnifying glass for the search-links dropdown trigger. Inherits `currentColor`.
const SEARCH_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.35-4.35"/></svg>"##;

/// Folder glyph shown on every node row. Inherits `currentColor`.
const FOLDER_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"##;

/// A music note shown on each audio-file row, so a file reads differently from the
/// folder rows around it. Inherits `currentColor`.
const MUSIC_SVG: &str = r##"<svg class="file-glyph" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>"##;

/// Check mark for the "no gaps in this root" state. Inherits `currentColor`.
const CHECK_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M5 13l4 4L19 7"/></svg>"##;

/// Circled exclamation for a scan or write error. Inherits `currentColor`.
const ERROR_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M12 8v4M12 16h.01"/></svg>"##;

/// A thin × for the filter's clear button: two diagonal strokes, no circle, in
/// `currentColor` so it follows the button's muted-to-base hover color.
const CLEAR_SVG: &str = r##"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>"##;

/// The favicon as an inline SVG data URI, so the tab gets an identity and the
/// browser stops requesting `/favicon.ico`. The "book wearing headphones" glyph
/// on its own, no backdrop. It draws in `currentColor`, and an embedded `<style>`
/// binds that to indigo `%23605dff` on light tab strips and a lighter indigo
/// `%23c7c5ff` on dark ones via `prefers-color-scheme`, so the mark keeps its
/// contrast either way. (Chrome, Firefox, and Edge honor the media query inside a
/// favicon; Safari ignores it and shows the light-mode indigo throughout.) The
/// source art lives at `assets/brand/favicon.svg`; keep it, `BRAND_SVG`, and the
/// source art in sync if the mark changes.
const FAVICON_HREF: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-linecap='round' stroke-linejoin='round'%3E%3Cstyle%3Esvg{color:%23605dff}@media(prefers-color-scheme:dark){svg{color:%23c7c5ff}}%3C/style%3E%3Cpath d='M4.5 14v-2a7.5 7.5 0 0 1 15 0v2' stroke-width='2'/%3E%3Crect x='3' y='13' width='3.2' height='6' rx='1.6' fill='currentColor' stroke='none'/%3E%3Crect x='17.8' y='13' width='3.2' height='6' rx='1.6' fill='currentColor' stroke='none'/%3E%3Cpath d='M12 11.8c-1.2-.85-3-.85-4.2 0v4.8c1.2-.85 3-.85 4.2 0c1.2-.85 3-.85 4.2 0v-4.8c-1.2-.85-3-.85-4.2 0z' stroke-width='1.4'/%3E%3Cpath d='M12 11.8v4.8' stroke-width='1.2'/%3E%3C/svg%3E";

/// The brand mark drawn inline at the head of the navbar, the same "book wearing
/// headphones" glyph as the favicon. It draws in `currentColor`, which the navbar
/// binds to the primary indigo so the mark follows the theme, and is `aria-hidden`
/// since the `<h1>` already names the app. Same art as `FAVICON_HREF` and
/// `assets/brand/favicon.svg`; keep the three in sync if the mark changes.
const BRAND_SVG: &str = r##"<svg class="brand-mark" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4.5 14v-2a7.5 7.5 0 0 1 15 0v2" stroke-width="2"/><rect x="3" y="13" width="3.2" height="6" rx="1.6" fill="currentColor" stroke="none"/><rect x="17.8" y="13" width="3.2" height="6" rx="1.6" fill="currentColor" stroke="none"/><path d="M12 11.8c-1.2-.85-3-.85-4.2 0v4.8c1.2-.85 3-.85 4.2 0c1.2-.85 3-.85 4.2 0v-4.8c-1.2-.85-3-.85-4.2 0z" stroke-width="1.4"/><path d="M12 11.8v4.8" stroke-width="1.2"/></svg>"##;

/// Vertical three-dot "more actions" glyph for the mobile per-row menu trigger.
/// Inherits `currentColor`.
const KEBAB_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="12" cy="19" r="2"/></svg>"##;

/// A "no entry" sign (circle with a horizontal bar) for the sheet's "Mark as
/// None" row. Shown only inside the mobile sheet. Inherits `currentColor`.
const NO_ENTRY_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M7 12h10"/></svg>"##;

/// A book with a small check, marking that this audiobook's ebook is accounted
/// for somewhere else rather than missing. Shown on the sheet's "Ebook
/// elsewhere" button. Inherits `currentColor`.
const EBOOK_ELSEWHERE_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H19a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1H6.5a1 1 0 0 1 0-5H20"/><path d="m9 9.5 2 2 4-4"/></svg>"##;

/// The gaps-only / show-all view control for the navbar: a two-segment control.
/// The segment for the current view is inert and marked `aria-current`; the other
/// is a GET link that navigates to its view. Switching reshapes every root, so it
/// is a full-page navigation, and the choice is not persisted.
fn view_toggle(mode: ViewMode) -> Markup {
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

/// The navbar settings control: a cog that opens a popover holding the
/// confirm-before-marking toggle, the theme choice, and a read-only keyboard
/// shortcuts reference. The native popover API drives open/close; the controls'
/// behavior lives in `app.js`, which also opens this panel on the `?` key. The
/// theme segments and the switch render with their default state (System,
/// confirm on); `app.js` reconciles them against localStorage once it runs. The
/// shortcuts section hides on mobile, where there is no keyboard.
fn settings_menu() -> Markup {
    html! {
        button.btn.btn-ghost.btn-square.settings-cog type="button"
            aria-label="Settings"
            title="Settings"
            aria-haspopup="menu"
            popovertarget="settings-panel" { (PreEscaped(COG_SVG)) }
        div.settings-panel id="settings-panel" popover="auto" aria-label="Settings" {
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
            div.settings-row.settings-row-theme {
                span.settings-label { "Theme" }
                div.segmented role="group" aria-label="Theme" {
                    button.segment type="button" data-theme-choice="light" { "Light" }
                    button.segment type="button" data-theme-choice="dark" { "Dark" }
                    button.segment.segment-active type="button"
                        data-theme-choice="system" aria-current="true" { "System" }
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

/// The navbar filter input, hidden until `app.js` reveals it (the connection
/// banner's hidden-until-ready pattern) so the no-JS page never shows a filter that
/// cannot run. The `/` key focuses it and Escape clears it; both live in `app.js`.
/// A themed clear button sits after the input, hidden until the box holds text.
fn search_box() -> Markup {
    html! {
        div.search id="search" hidden {
            (PreEscaped(SEARCH_SVG))
            input.search-input id="search-input" type="search"
                placeholder="Filter folders" aria-label="Filter folders"
                autocomplete="off";
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
fn search_empty() -> Markup {
    html! {
        p.search-empty id="search-empty" role="status" aria-live="polite" hidden {
            "No folders match your filter."
        }
    }
}

/// The marker-write confirmation, rendered once at the page level so it survives
/// the htmx section swaps. `app.js` fills the title, folder, file chip, confirm
/// label, and the matching marker glyph from the button that fired, then opens
/// it. Without JS the dialog never opens and writes proceed as before.
fn confirm_dialog() -> Markup {
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

/// The rescan placeholder. It overlays `#roots` while an in-place rescan is in
/// flight: htmx adds `htmx-request` to it for the duration (its bundled indicator
/// styles fade it in), and the shimmer reads as work in progress. Hidden
/// otherwise, and never shown on the no-JS path, which does a full reload.
fn scan_skeleton() -> Markup {
    html! {
        div.scan-skeleton.htmx-indicator id="scan-skeleton" aria-hidden="true" {
            @for _ in 0..5 {
                div.sk-row { span.sk.sk-icon {} span.sk.sk-name {} }
            }
        }
    }
}

/// The connection-status banner: a polite live region pinned to the top of the
/// page, hidden until app.js reveals it and sets a state class. The state copy is
/// carried as data attributes so it is defined (and tested) here in one place; the
/// client only chooses which message to show.
fn conn_banner() -> Markup {
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
fn toast() -> Markup {
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

/// The rotating folder caret used on collapsible rows.
fn chevron() -> Markup {
    html! { (PreEscaped(CHEVRON_SVG)) }
}

/// The folder glyph used on every node row (every node is a folder).
fn folder_icon() -> Markup {
    html! { (PreEscaped(FOLDER_SVG)) }
}

/// The root sections in order. Shared by the full page and the htmx rescan
/// response, which swaps just this list into `#roots`.
pub(crate) fn roots(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    html! {
        @for (root, section) in view.iter().enumerate() {
            (render_section(section, root, None, links, mode))
        }
    }
}

pub(crate) fn page(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Missing Ebooks" }
                link rel="icon" href=(FAVICON_HREF);
                script { (PreEscaped(PREPAINT_THEME_JS)) }
                link rel="stylesheet" href="/static/app.css";
            }
            body {
                (conn_banner())
                nav.navbar {
                    // The title is a home link: a plain GET to "/" that survives the
                    // htmx swaps and works without JS, landing on the default gaps-only
                    // view with no filter, the conventional reset for a wordmark.
                    h1 { a href="/" { (PreEscaped(BRAND_SVG)) "Missing Ebooks" } }
                    // The spacer sits right after the title so the title alone pins
                    // left and everything else groups on the right. Mobile reorders
                    // every control by flex `order`, so this DOM move is desktop-only.
                    span.spacer {}
                    (search_box())
                    (view_toggle(mode))
                    (settings_menu())
                    form method="post" action="/rescan"
                        hx-post="/rescan" hx-target="#roots" hx-swap="innerHTML"
                        hx-indicator="#scan-skeleton, #rescan-btn"
                        hx-disabled-elt="#rescan-btn" {
                        input type="hidden" name="view" value=(mode.as_query());
                        // The button sits in the nav, outside the swapped #roots, so htmx
                        // keeps it across the request: hx-disabled-elt locks it so a second
                        // click cannot double-scan, and the disabled state dims it (app.css).
                        // The label stays put; both clear once the swap settles.
                        button.btn.btn-primary id="rescan-btn" type="submit" { "Rescan" }
                    }
                }
                (gap_summary(view))
                div.roots-wrap {
                    main id="roots" {
                        (roots(view, links, mode))
                    }
                    (scan_skeleton())
                    (search_empty())
                }
                (confirm_dialog())
                (toast())
                script src="/static/htmx.min.js" {}
                script src="/static/app.js" {}
            }
        }
    }
}

/// Count the gaps (`needs_ebook()` nodes) anywhere in a forest. Drives the root
/// summary badge so a collapsed root still tells you how much is unresolved.
fn count_gaps(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| usize::from(n.needs_ebook()) + count_gaps(&n.children))
        .sum()
}

/// Total gaps across all roots: the sum of `count_gaps` over each forest. `Clean`
/// and `Error` roots contribute nothing. Feeds the summary hero and the session
/// bar's load-time baseline.
fn total_gaps(view: &FlaggedView) -> usize {
    view.iter()
        .map(|section| match &section.state {
            RootState::Forest(nodes) => count_gaps(nodes),
            RootState::Clean | RootState::Error(_) => 0,
        })
        .sum()
}

/// A root's short label: the last non-empty path segment, for the per-root chips.
fn root_label(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|seg| !seg.is_empty())
        .unwrap_or(path)
}

/// "gap" / "gaps".
fn gap_word(n: usize) -> &'static str {
    if n == 1 { "gap" } else { "gaps" }
}

/// The gap summary strip, rendered between the navbar and the roots and computed
/// from the `FlaggedView` already on hand, so it needs no scanner change. The hero
/// gap total, a session coverage readout with its progress bar, and optional
/// per-root chips for a multi-root setup. `app.js` keeps the hero and readout
/// current from the DOM as marks land; this render is the first paint and the no-JS
/// view. `data-gaps-at-load` seeds the session bar's baseline.
fn gap_summary(view: &FlaggedView) -> Markup {
    let total = total_gaps(view);
    html! {
        section.gap-summary id="gap-summary" data-gaps-at-load=(total) {
            // Both end-states render; `app.js` toggles `hidden` as the live total
            // crosses zero so the strip converges on what a reload would show, and so
            // an undo back from the last mark can bring the hero and bar back.
            p.gap-summary-clear id="gap-summary-clear" hidden[total != 0] {
                (PreEscaped(CHECK_SVG))
                span { "All clear. No gaps in your library." }
            }
            div.gap-summary-head id="gap-summary-head" hidden[total == 0] {
                div.gap-hero {
                    span.gap-hero-num id="gap-total" { (total) }
                    span.gap-hero-label { (gap_word(total)) " to fill" }
                }
                (session_bar(view, total))
            }
            @if view.len() > 1 {
                div.gap-chips id="gap-chips" {
                    @for (root, section) in view.iter().enumerate() {
                        (root_chip(root, section))
                    }
                }
            }
        }
    }
}

/// One per-root chip: the root's short label and its own gap count, shown only in
/// a multi-root setup. A covered root reads zero; an error root reads "scan error".
/// The `data-root` hook lets the client recompute update each chip independently.
fn root_chip(root: usize, section: &RootSection) -> Markup {
    html! {
        @match &section.state {
            RootState::Forest(nodes) => {
                @let n = count_gaps(nodes);
                span.gap-chip data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { (n) }
                }
            }
            RootState::Clean => {
                span.gap-chip.gap-chip-clean data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { "0" }
                }
            }
            RootState::Error(_) => {
                span.gap-chip.gap-chip-error data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { "scan error" }
                }
            }
        }
    }
}

/// The session coverage block beside the hero: a label naming the roots it spans, a
/// readout of gaps resolved this sitting over the count at load
/// (`{resolved} of {baseline} audiobooks · {pct}%`), and the progress bar. Renders
/// at zero (`0 of {total}`, empty bar); `app.js` rewrites the readout and fills the
/// bar as marks land, and resets the baseline on a rescan. The numbers aggregate
/// every root, so the label lists every root to make that scope explicit. A
/// `progressbar` so the value is announced; the fill transition is dropped under
/// reduced motion in CSS.
fn session_bar(view: &FlaggedView, total: usize) -> Markup {
    html! {
        div.gap-session {
            div.gap-session-head {
                p.gap-session-label {
                    "Coverage in "
                    span.gap-session-roots { (root_names(view)) }
                }
                p.gap-session-readout {
                    span.gap-session-num id="gap-resolved" { "0" }
                    " of "
                    span.gap-session-num id="gap-baseline" { (total) }
                    " audiobooks · "
                    span.gap-session-num id="gap-pct" { "0" } "%"
                }
            }
            div.gap-bar role="progressbar"
                aria-label="Gaps resolved this session"
                aria-valuemin="0" aria-valuemax=(total) aria-valuenow="0" {
                span.gap-bar-fill id="gap-bar-fill" {}
            }
        }
    }
}

/// The root names the coverage readout spans, comma-joined, so its all-roots scope
/// reads plainly. One root gives "Library"; several give "Library A, Library B".
fn root_names(view: &FlaggedView) -> String {
    view.iter()
        .map(|section| root_label(&section.path))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The badge shown on a root's summary: the gap count, a clean check, or a scan
/// error. In show-all the forest also holds covered nodes; only gaps are counted.
fn root_badge(state: &RootState) -> Markup {
    html! {
        @match state {
            RootState::Forest(nodes) => {
                @let n = count_gaps(nodes);
                @if n == 0 {
                    span.root-badge.root-badge-clean { "\u{2713} no gaps" }
                } @else if n == 1 {
                    span.root-badge.root-badge-gaps { "1 gap" }
                } @else {
                    span.root-badge.root-badge-gaps { (n) " gaps" }
                }
            }
            RootState::Clean => {
                span.root-badge.root-badge-clean { "\u{2713} no gaps" }
            }
            RootState::Error(_) => {
                span.root-badge.root-badge-error { "scan error" }
            }
        }
    }
}

pub(crate) fn render_section(
    section: &RootSection,
    root: usize,
    error: Option<&str>,
    links: &[SearchLink],
    mode: ViewMode,
) -> Markup {
    let counter = std::cell::Cell::new(0usize);
    html! {
        section.card.root data-root=(root) {
            details.root-fold open {
                summary.root-head {
                    (chevron())
                    h2 { (section.path) }
                    span.spring {}
                    (root_badge(&section.state))
                }
                @if let Some(message) = error {
                    div.alert.alert-error { (PreEscaped(ERROR_SVG)) span { (message) } }
                }
                @match &section.state {
                    RootState::Forest(nodes) => {
                        @if nodes.is_empty() {
                            // Show-all yields an empty forest only for a root with no
                            // folders at all. Gaps-only sets Clean instead.
                            div.empty { span { "Nothing here" } }
                        } @else {
                            ul.menu {
                                @for node in nodes { (render_node(node, root, links, mode, &counter, 0)) }
                            }
                        }
                    }
                    RootState::Clean => {
                        div.empty { (PreEscaped(CHECK_SVG)) span { "No missing ebooks in this root" } }
                    }
                    RootState::Error(message) => {
                        div.alert.alert-error {
                            (PreEscaped(ERROR_SVG)) span { "Could not scan this root: " (message) }
                        }
                    }
                }
            }
        }
    }
}

/// A standalone error card for a root whose section could not be looked up (its
/// index is out of range), carrying the same alert the in-fold error uses. Used by
/// the failed-write path when the view has no section to render into.
pub(crate) fn error_section(root: usize, message: &str) -> Markup {
    html! {
        section.card.root data-root=(root) {
            div.alert.alert-error { (PreEscaped(ERROR_SVG)) span { (message) } }
        }
    }
}

/// The show-all status marker for a row: a success check on a covered folder. Gaps
/// are already flagged by the amber icon and the badge, and plain containers need
/// no marker, so neither gets one. Rendered only in show-all mode.
fn status_icon(node: &Node) -> Markup {
    html! {
        @if !node.missing_ebook {
            span.status title="covered" { (PreEscaped(CHECK_SVG)) }
        }
    }
}

/// The covering ebook and marker filenames for a covered row, in muted text just
/// after the status check. Show-all only; empty for gaps and for folders covered
/// from above, so nothing renders there.
fn cover_files_span(node: &Node, mode: ViewMode) -> Markup {
    html! {
        @if mode == ViewMode::All && !node.cover_files.is_empty() {
            span.cover-files title="covering files" { (node.cover_files.join(", ")) }
        }
    }
}

/// The structural-smell microlabel for a flagged row. A folder that directly holds
/// audio and also has gap subfolders reads as mixed (a book and a shelf at once); a
/// flagged leaf with no container around it (depth 0, including the root-itself gap)
/// reads as loose. A gap filed under a container at depth 1 or deeper gets nothing.
/// Non-flagged nodes never reach a true branch.
fn smell_label(node: &Node, depth: usize) -> Markup {
    html! {
        @if node.needs_ebook() {
            @if !node.children.is_empty() {
                span.smell.smell-mixed { "holds audio + subfolders" }
            } @else if depth == 0 {
                span.smell.smell-loose { "loose at top" }
            }
        }
    }
}

/// The muted "N files" count for a flagged row. Gated on a gap so it appears only
/// where the file disclosure does, never on a covered or container row.
fn file_count(node: &Node) -> Markup {
    html! {
        @if node.needs_ebook() && !node.audio_files.is_empty() {
            @let n = node.audio_files.len();
            span.file-count {
                @if n == 1 { "1 file" } @else { (n) " files" }
            }
        }
    }
}

/// The audio-file rows for a flagged folder, each a muted, non-actionable line with a
/// music glyph. Emits nothing on a container or covered row, so it is safe to call
/// unconditionally inside the container branch where only a mixed node has files.
fn file_rows(node: &Node) -> Markup {
    html! {
        @if node.needs_ebook() {
            @for name in &node.audio_files {
                li.file-row {
                    (PreEscaped(MUSIC_SVG))
                    span.file-name { (name) }
                }
            }
        }
    }
}

fn render_node(
    node: &Node,
    root: usize,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
    depth: usize,
) -> Markup {
    // A covered row dims only in show-all; gaps-only never holds covered nodes.
    let covered = mode == ViewMode::All && !node.missing_ebook;
    // Buttons and links appear only where there is a gap to act on.
    let act = node.has_gap_within();
    html! {
        @if node.children.is_empty() {
            @if node.needs_ebook() {
                // A flagged leaf: an expandable row whose audio files sit hidden under
                // it until opened. It renders as a <summary> like a flagged container,
                // a shape app.js already handles (see rowOf in app.js).
                li {
                    details.node-files {
                        summary.row.flagged {
                            (chevron())
                            (folder_icon())
                            span.name { (node.name) }
                            span.badge.badge-warning title="needs ebook" { "needs ebook" }
                            (smell_label(node, depth))
                            (file_count(node))
                            @if mode == ViewMode::All { (status_icon(node)) }
                            (cover_files_span(node, mode))
                            span.spring {}
                            @if act {
                                (row_actions(root, &node.rel_path, &node.name, links, mode, counter))
                            }
                        }
                        ul.files { (file_rows(node)) }
                    }
                }
            } @else {
                // A non-flagged leaf (a covered or plain folder in show-all) stays a
                // static row, exactly as before.
                li {
                    div.row.covered[covered] {
                        span.leaf-pad {}
                        (folder_icon())
                        span.name { (node.name) }
                        @if mode == ViewMode::All { (status_icon(node)) }
                        (cover_files_span(node, mode))
                        span.spring {}
                    }
                }
            }
        } @else {
            li {
                details open {
                    summary.row
                        .flagged[node.needs_ebook()]
                        .covered[covered]
                        .container-top[!node.needs_ebook() && depth == 0]
                        .container-nested[!node.needs_ebook() && depth > 0] {
                        (chevron())
                        (folder_icon())
                        span.name { (node.name) }
                        @if node.needs_ebook() { span.badge.badge-warning title="needs ebook" { "needs ebook" } }
                        (smell_label(node, depth))
                        (file_count(node))
                        @if mode == ViewMode::All { (status_icon(node)) }
                        (cover_files_span(node, mode))
                        span.spring {}
                        @if act {
                            (row_actions(root, &node.rel_path, &node.name, links, mode, counter))
                        }
                    }
                    ul {
                        (file_rows(node))
                        @for child in &node.children { (render_node(child, root, links, mode, counter, depth + 1)) }
                    }
                }
            }
        }
    }
}

/// The per-row action cluster: a kebab trigger plus the marker buttons and the
/// search links, wrapped in a group that doubles as a native popover. On desktop
/// the trigger is hidden and the group is `display: contents`, so its children
/// flow inline in the row as before; on mobile the kebab opens the group as a
/// bottom action sheet over a dimmed backdrop. The browser provides the toggle,
/// one-open-at-a-time, light-dismiss, and Esc.
fn row_actions(
    root: usize,
    rel: &str,
    name: &str,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    let group_id = next_id("acts", root, counter);
    html! {
        button.actions-trigger type="button"
            aria-label="Actions"
            aria-haspopup="menu"
            popovertarget=(group_id)
            onclick="event.stopPropagation()" { (PreEscaped(KEBAB_SVG)) }
        div.actions-group id=(group_id) popover="auto" aria-label=(name) {
            div.sheet-header {
                span.sheet-grip aria-hidden="true" {}
                span.sheet-title { (name) }
            }
            (marker_buttons(root, rel, name, mode))
            (search_links(links, name, root, counter))
        }
    }
}

fn marker_buttons(root: usize, rel: &str, name: &str, mode: ViewMode) -> Markup {
    // In gaps-only the marked folder leaves the list, so app.js collapses its row and
    // the section swap waits for that to play. In show-all the row stays and flips to
    // covered in place, so the swap is immediate; the row's reserved min-height keeps
    // the flip from shifting the rows below.
    let swap = match mode {
        ViewMode::GapsOnly => "outerHTML swap:250ms",
        ViewMode::All => "outerHTML",
    };
    html! {
        form.mark.actions hx-target="closest section.root" hx-swap=(swap) {
            input type="hidden" name="root" value=(root);
            input type="hidden" name="rel" value=(rel);
            input type="hidden" name="view" value=(mode.as_query());
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"no_ebook"}"#)
                data-confirm-action="Mark as None"
                data-confirm-file=".no_ebook"
                data-confirm-folder=(name)
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(NO_ENTRY_SVG)) }
                    span.label-long { "Mark as None" }
                    span.label-short { "None" }
                }
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"ebook_elsewhere"}"#)
                data-confirm-action="Ebook elsewhere"
                data-confirm-file=".ebook_elsewhere"
                data-confirm-folder=(name)
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(EBOOK_ELSEWHERE_SVG)) }
                    span.label-long { "Ebook elsewhere" }
                    span.label-short { "Elsewhere" }
                }
        }
    }
}

/// Next DOM-safe id with the given prefix for a row's popover or action group.
/// The relative path can hold slashes and spaces, so a root index plus a
/// per-render counter is used instead. The counter is reset per
/// `render_section`, so ids are unique within one render.
fn next_id(prefix: &str, root: usize, counter: &std::cell::Cell<usize>) -> String {
    let n = counter.get();
    counter.set(n + 1);
    format!("{prefix}-{root}-{n}")
}

fn search_links(
    links: &[SearchLink],
    name: &str,
    root: usize,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    html! {
        @if !links.is_empty() {
            @let query = urlencoding::encode(&clean_query(name)).into_owned();
            @let id = next_id("links", root, counter);
            span.links {
                span.sheet-divider { "Search" }
                button.btn.btn-outline.btn-xs.links-toggle type="button"
                    popovertarget=(id)
                    aria-label="Search for this book"
                    title="Search links"
                    onclick="event.stopPropagation()" { (PreEscaped(SEARCH_SVG)) }
                div.links-menu popover="auto" id=(id) onclick="event.stopPropagation()" {
                    @for link in links {
                        a href=(link.url.replace("{query}", &query))
                            target="_blank" rel="noopener noreferrer" {
                                span.sheet-icon { (PreEscaped(SEARCH_SVG)) }
                                (link.label)
                            }
                    }
                }
            }
        }
    }
}
