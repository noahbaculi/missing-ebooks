//! Page shell and navbar/popover chrome. Imported by `web::render`, never
//! the reverse: the chrome and the tree never share a module, so the shell
//! takes the body as a `Markup` parameter and never touches domain types
//! like `Node` or the per-section packaged view.

use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::tree::ViewMode;

/// The favicon as an inline SVG data URI, so the tab gets an identity and the
/// browser stops requesting `/favicon.ico`. The "book wearing headphones" glyph,
/// no backdrop. It draws in `currentColor`. An embedded `<style>` binds that to
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

/// The navbar help control: a "?" icon button beside the settings cog. Clicking
/// it toggles the intro card: it shows the card when hidden and hides it when
/// showing, so dismissing the card is not a one-way door. Behavior lives in
/// `app.js`, bound by the id. The button takes its own job rather than the `?`
/// key, which already opens the settings popover.
///
/// The `aria-controls` pair points at the card the button toggles, and
/// `aria-expanded` advertises the toggle's current state. The initial value
/// matches the default render (card shown), and `app.js` reconciles it against
/// the per-device dismissal flag on `DOMContentLoaded` and keeps it in sync as
/// the visitor toggles, so screen-reader users hear the right state.
pub(super) fn help_button() -> Markup {
    html! {
        button.btn.btn-ghost.btn-square.intro-help id="intro-help" type="button"
            aria-label="How this works"
            title="How this works"
            aria-controls="intro-card"
            aria-expanded="true" { (PreEscaped(include_str!("../../assets/svg/help.svg"))) }
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

/// One server copy of the warn glyph for the inline mark-failure strip, sourced
/// from `assets/svg/warning.svg` and held inside a `<template>` so it never
/// renders. `app.js` clones it per failure rather than carrying a duplicate SVG
/// string, matching how every other glyph on the page is served.
pub(super) fn mark_warn_template() -> Markup {
    html! {
        template id="mark-warn-tpl" {
            (PreEscaped(include_str!("../../assets/svg/warning.svg")))
        }
    }
}

/// The dismissible intro card, above the coverage strip in every deployment. It
/// orients a cold visitor who landed on the tool without reading the README: it
/// names the concept (each row is an audiobook folder with no ebook) and the
/// actions (search links, or the No ebook / Elsewhere markers), naming the row
/// controls so the tree reads. Dismissal is a per-device `localStorage` flag. The
/// pre-paint bootstrap hides this card before first paint when the flag is set, so
/// a repeat visitor never sees it flash. The help button toggles it back.
/// The title carries that same help-circle glyph to link the card to the button.
pub(super) fn intro_card() -> Markup {
    html! {
        section.intro-card id="intro-card" aria-labelledby="intro-title" {
            button.intro-dismiss id="intro-dismiss" type="button" aria-label="Dismiss introduction" { "\u{00D7}" }
            h2.intro-title id="intro-title" {
                (PreEscaped(include_str!("../../assets/svg/help.svg")))
                span { "Audiobooks missing an ebook" }
            }
            p.intro-body {
                "Each row below is an audiobook folder with no ebook beside it. "
                "Use the search links to look for the ebook, or mark the folder "
                span.intro-mark { "No ebook" }
                " if none exists or "
                span.intro-mark { "Elsewhere" }
                " if it lives in another folder."
            }
        }
    }
}

/// The page footer, present in every deployment. Carries the brand mark, the
/// product name, the crate version, and a link to the source. Because it ships
/// in the core shell, it gives a demo visitor a second path home alongside the
/// banner CTA, and a self-hoster a way back to the source. The version is the
/// compile-time `CARGO_PKG_VERSION`, so the shell stays free of domain types
/// and runtime state.
pub(super) fn footer() -> Markup {
    html! {
        footer.site-footer {
            span.footer-brand {
                (PreEscaped(include_str!("../../assets/svg/brand.svg")))
                span.footer-name { "Missing Ebooks" }
                span.footer-version { "v" (env!("CARGO_PKG_VERSION")) }
            }
            nav.footer-links aria-label="About" {
                a href="https://github.com/noahbaculi/missing-ebooks" { "GitHub" }
            }
        }
    }
}

/// The HTML document shell: head, noscript notice, connection banner, SSE
/// listener, navbar, confirm dialog, toast machinery, and the script tags.
/// Wraps a prebuilt `body` markup (typically the gap summary plus the
/// `#roots` block, assembled inside `render::page`). Pure shell: takes
/// no domain types, only the current `ViewMode` for the navbar and the SSE
/// query string.
///
/// `poll_interval_seconds` gates the client-driven refresh path. When `> 0`,
/// the shell emits a `<div id="poll-root">` marker that `app.js` reads on
/// load and uses to poll `/refresh?view=...` while the tab is visible. When
/// `0`, the shell keeps the legacy SSE mount so behavior is unchanged until
/// the rollout task flips the default. See ADR-0034 (added in a later task).
pub(crate) fn page(mode: ViewMode, poll_interval_seconds: u64, body: &Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Missing Ebooks" }
                link rel="icon" href=(FAVICON_HREF);
                // Pre-paint bootstrap. See assets/prepaint.js. The
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
                @if poll_interval_seconds > 0 {
                    // Client-driven refresh. See ADR-0034. `app.js` reads
                    // data-poll-interval and data-view on load and starts a
                    // setInterval gated on document.visibilityState.
                    div #poll-root
                        data-poll-interval=(poll_interval_seconds)
                        data-view=(mode.as_query()) {}
                } @else {
                    // Legacy SSE mount. Deleted in the task that removes autosync.
                    div hx-ext="sse"
                        sse-connect=(format!("/events?view={}", mode.as_query()))
                        sse-swap="section,snapshot" {}
                }
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
                    (help_button())
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
                (intro_card())
                (body)
                (confirm_dialog())
                (toast())
                (mark_warn_template())
                (footer())
                script src="/static/htmx.min.js" {}
                script src="/static/htmx-sse.js" {}
                script src="/static/app.js" {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maud::html;

    /// A stub body for tests that only care about the page shell, not what
    /// sits inside it.
    fn stub_body() -> maud::Markup {
        html! { div #stub {} }
    }

    #[test]
    fn page_emits_the_poll_marker_when_interval_is_nonzero() {
        let html = page(ViewMode::GapsOnly, 10, &stub_body()).into_string();
        assert!(html.contains(r#"id="poll-root""#));
        assert!(html.contains(r#"data-poll-interval="10""#));
        assert!(html.contains(r#"data-view="gaps""#));
        // The SSE mount is suppressed when polling is active.
        assert!(!html.contains(r#"hx-ext="sse""#));
    }

    #[test]
    fn page_emits_the_sse_mount_when_interval_is_zero() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        assert!(html.contains(r#"hx-ext="sse""#));
        assert!(!html.contains(r#"id="poll-root""#));
    }

    #[test]
    fn index_links_an_inline_favicon() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // An inline SVG data-URI favicon, so the browser stops requesting
        // /favicon.ico and the tab gets an identity.
        assert!(html.contains(r#"rel="icon""#));
        assert!(html.contains("data:image/svg+xml,"));
        // A backdrop-less audiobook glyph that recolors with the OS theme: indigo
        // on light tab strips, lighter on dark. Pin the indigo and the
        // prefers-color-scheme rule so a revert to the old glyph or a single static
        // color is caught. The 22x22 rect was the dropped rounded tile.
        assert!(html.contains("%23605dff"));
        assert!(html.contains("prefers-color-scheme:dark"));
        assert!(!html.contains("width='22'"));
    }

    #[test]
    fn index_links_the_stylesheet_and_inits_the_theme() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The external stylesheet replaces the old inline <style> block.
        assert!(html.contains(r#"href="/static/app.css""#));
        // The pre-paint theme script is present and reads the OS preference.
        assert!(html.contains("prefers-color-scheme"));
        // The pre-paint bootstrap also resolves both depth preferences, so a reader
        // who opted out of an effect never flashes it before app.js runs.
        assert!(html.contains("boldTopFolder"));
        assert!(html.contains("italicNestedFolders"));
        assert!(html.contains("dataset.boldTop"));
        assert!(html.contains("dataset.italicNested"));
        // The theme toggle moved into the settings menu: a labelled cog, with the
        // theme choices inside the panel.
        assert!(html.contains(r#"aria-label="Settings""#));
        assert!(html.contains(r#"data-theme-choice="system""#));
    }

    #[test]
    fn prepaint_bootstrap_handles_the_accent_preference() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The inline pre-paint script reads the accent key and derives the ink
        // before first paint, so a custom accent never flashes the default ink.
        assert!(html.contains("getItem('accent')"));
        assert!(html.contains("deriveWarningInk"));
        assert!(html.contains("--color-warning-text"));
        // The default writes no override, so it must match the shipped amber.
        assert!(html.contains("'#f5a524'"));
    }

    #[test]
    fn prepaint_bootstrap_hides_a_dismissed_intro_card() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The pre-paint bootstrap reads the per-device dismissal flag and reflects
        // it onto <html> before first paint, so a repeat visitor never flashes the
        // card. Same dataset pattern as the depth preferences.
        assert!(html.contains("introDismissed"));
        assert!(html.contains("dataset.intro"));
    }

    #[test]
    fn page_carries_a_noscript_notice() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The UI requires JavaScript. A <noscript> strip is the one thing a
        // scripting-disabled visitor sees.
        assert!(html.contains("<noscript>"));
        assert!(html.contains(r#"<div class="noscript-notice">"#));
        assert!(html.contains("needs JavaScript to run"));
    }

    #[test]
    fn page_loads_htmx_htmx_sse_and_app_scripts() {
        // The body-end <script> half of the original
        // `index_renders_the_marker_buttons_and_script` (the marker-button
        // half landed in render::tests during the cluster E migration).
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        assert!(html.contains(r#"src="/static/htmx.min.js""#));
        assert!(html.contains(r#"src="/static/htmx-sse.js""#));
        assert!(html.contains(r#"src="/static/app.js""#));
    }

    #[test]
    fn navbar_renders_a_settings_cog_with_theme_and_confirm_controls() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // A labelled cog opens the settings panel via the native popover API.
        assert!(html.contains(r#"class="btn btn-ghost btn-square settings-cog""#));
        assert!(html.contains(r#"aria-label="Settings""#));
        assert!(html.contains(r#"popovertarget="settings-panel""#));
        assert!(html.contains(r#"id="settings-panel""#));
        // The theme segmented control offers all three states.
        assert!(html.contains(r#"data-theme-choice="light""#));
        assert!(html.contains(r#"data-theme-choice="dark""#));
        assert!(html.contains(r#"data-theme-choice="system""#));
        // The Theme control's header reuses the .settings-head styling, like the
        // folder-depth group.
        assert!(html.contains(r#"<div class="settings-head">Theme</div>"#));
        // The confirm-before-marking switch.
        assert!(html.contains(r#"id="confirm-toggle""#));
        // The panel orders the theme control first, then the confirm switch, then
        // the two folder-depth styling switches under their header, bold then italic.
        assert!(html.contains("Folder depth styling"));
        assert!(html.contains(r#"id="bold-top-toggle""#));
        assert!(html.contains(r#"id="italic-nested-toggle""#));
        let theme_at = html.find(r#"data-theme-choice="light""#).unwrap();
        let confirm_at = html.find(r#"id="confirm-toggle""#).unwrap();
        let bold_at = html.find(r#"id="bold-top-toggle""#).unwrap();
        let italic_at = html.find(r#"id="italic-nested-toggle""#).unwrap();
        assert!(
            theme_at < confirm_at && confirm_at < bold_at && bold_at < italic_at,
            "the panel should order theme, then confirm, then the depth switches bold-then-italic"
        );
        // The panel title sits below the theme selector, heading the rest.
        let settings_head_at = html
            .find(r#"<div class="settings-head">Settings</div>"#)
            .unwrap();
        assert!(
            theme_at < settings_head_at && settings_head_at < confirm_at,
            "the Settings title should sit below the theme selector and above the confirm switch"
        );
    }

    #[test]
    fn panel_renders_the_accent_color_control() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // A regular-weight row label, not a section header.
        assert!(html.contains(r#"<span class="settings-label">Accent Color</span>"#));
        // The native color picker, defaulting to the shipped amber.
        assert!(html.contains(r#"id="accent-input""#));
        assert!(html.contains(r#"type="color""#));
        assert!(html.contains(r##"value="#f5a524""##));
        // The three preset quick-pick dots. Amber is the default, so it is not a
        // preset, so the dots offer the alternatives.
        assert!(html.contains(r##"data-accent="#06b6d4""##));
        assert!(html.contains(r##"data-accent="#c2410c""##));
        assert!(html.contains(r##"data-accent="#a21caf""##));
        // It sits inside the Theme section: after the theme choices, before the
        // Settings header.
        let theme_at = html.find(r#"data-theme-choice="system""#).unwrap();
        let accent_at = html.find(r#"id="accent-input""#).unwrap();
        let settings_head_at = html
            .find(r#"<div class="settings-head">Settings</div>"#)
            .unwrap();
        assert!(
            theme_at < accent_at && accent_at < settings_head_at,
            "the Accent Color row should sit inside the Theme section, below the theme choices and above the Settings header"
        );
    }

    #[test]
    fn the_view_control_marks_the_active_segment() {
        // Gaps-only is the active view. "All folders" is the link to the other view.
        let gaps = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        assert!(gaps.contains(r#"class="segmented""#));
        assert!(gaps.contains("Gaps only"));
        assert!(gaps.contains("All folders"));
        assert!(gaps.contains(r#"href="/?view=all""#));

        // Show-all is active. "Gaps only" links back to /.
        let all = page(ViewMode::All, 0, &stub_body()).into_string();
        assert!(all.contains(r#"href="/""#));
        assert!(all.contains(r#"aria-current="page""#));
    }

    #[test]
    fn navbar_renders_the_brand_mark_before_the_title() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The title is a home link wrapping the brand glyph and the wordmark. The
        // single assertion fixes the link, the inline mark, and its leading position.
        assert!(html.contains(r#"<h1><a href="/"><svg class="brand-mark""#));
        assert!(html.contains("Missing Ebooks"));
    }

    #[test]
    fn decorative_icons_are_hidden_from_assistive_tech() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The folder glyph renders on every node row. It must be hidden from the
        // a11y tree (it is paired with the folder name) and not be a tab stop.
        // The shell carries other icons that satisfy the same shape (cog, search,
        // check, ebook-elsewhere, no-entry), so calling `page` with a stub body
        // is enough.
        assert!(html.contains(r#"<svg class="icon" aria-hidden="true" focusable="false""#));
    }

    #[test]
    fn navbar_places_the_spacer_before_the_search_box() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The flexible spacer sits right after the title, so the title alone pins to the
        // left and the search box groups with the controls on the right.
        let spacer = html
            .find(r#"<span class="spacer">"#)
            .expect("spacer present");
        let search = html
            .find(r#"<div class="search""#)
            .expect("search box present");
        assert!(
            spacer < search,
            "the spacer should sit before the search box"
        );
    }

    #[test]
    fn index_renders_the_shortcuts_inside_the_settings_panel() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The shortcuts are a read-only section inside the settings popover.
        assert!(html.contains(r#"class="settings-shortcuts""#));
        assert!(html.contains("Keyboard shortcuts"));
        // The keys are spelled out for the reader.
        assert!(html.contains("<kbd>j</kbd>"));
        assert!(html.contains("Move between gaps"));
        // Enter leaves the filter box, the complement of / focusing it.
        assert!(html.contains("<kbd>Enter</kbd>"));
        assert!(html.contains("Exit the filter"));
    }

    #[test]
    fn navbar_renders_the_rescan_form_with_htmx_attrs() {
        // The navbar half of the original
        // `rescan_is_an_in_place_htmx_swap_with_a_progress_bar`. The scan-bar
        // id pin lives next to it as `scan_bar_carries_the_indicator_id`.
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // Rescan posts via htmx and swaps the fresh sections into #roots.
        assert!(html.contains(r#"hx-post="/rescan""#));
        assert!(html.contains(r##"hx-target="#roots""##));
        // The progress bar lights up as the indicator, and the button is disabled for
        // the request so a second click cannot fire a second scan.
        assert!(html.contains(r##"hx-indicator="#scan-bar, #rescan-btn""##));
        assert!(html.contains(r##"hx-disabled-elt="#rescan-btn""##));
        // The button keeps its constant "Rescan" label and locks via hx-disabled-elt.
        assert!(html.contains(r#"id="rescan-btn""#));
        assert!(html.contains("Rescan"));
        // Rescan is htmx-driven, not a native form submit: the button posts via
        // hx-post and the form carries no method/action.
        assert!(!html.contains(r#"action="/rescan""#));
        assert!(!html.contains(r#"method="post""#));
        assert!(html.contains(r#"id="rescan-btn" type="button" hx-post="/rescan""#));
    }

    #[test]
    fn scan_bar_carries_the_indicator_id() {
        // The bar is wired as the rescan indicator. The page shell embeds it via
        // `render::page`. Calling `scan_bar` directly pins the id without that detour.
        let html = scan_bar().into_string();
        assert!(html.contains(r#"id="scan-bar""#));
    }

    #[test]
    fn navbar_renders_the_disabled_filter_input_and_no_matches_line() {
        // The search box itself ships inside the navbar (called from `page`),
        // and the no-matches line is the standalone `search_empty` helper that
        // `render::page` drops next to the roots block.
        let box_html = search_box().into_string();
        // A filter input with an accessible name. The box renders visible from first
        // paint so it never reflows in. The input renders `disabled` and app.js clears
        // that once the tree and handler are wired, so the box is greyed but never a
        // dead box the user can type into before it works.
        assert!(box_html.contains(r#"id="search-input""#));
        assert!(box_html.contains(r#"aria-label="Filter folders""#));
        assert!(box_html.contains(r#"<div class="search" id="search">"#));
        assert!(box_html.contains(r#"autocomplete="off" disabled>"#));

        let empty_html = search_empty().into_string();
        // A polite "no matches" line, hidden until a query matches nothing.
        assert!(empty_html.contains(r#"id="search-empty""#));
        assert!(empty_html.contains(r#"aria-live="polite""#));
        assert!(empty_html.contains("No folders match"));
    }

    #[test]
    fn search_box_renders_a_hidden_themed_clear_button() {
        let html = search_box().into_string();
        // A labelled clear button sits in the filter box, hidden at first paint so it
        // only appears once the box holds text (app.js drives the toggle).
        assert!(html.contains(r#"id="search-clear""#));
        assert!(html.contains(r#"aria-label="Clear filter" hidden"#));
        // It carries a thin-× glyph, not a circle: two diagonal strokes.
        assert!(html.contains(r#"d="M6 6l12 12M18 6L6 18""#));
    }

    #[test]
    fn index_renders_the_hidden_connection_banner() {
        let html = conn_banner().into_string();
        // A polite live region, hidden until the connection JS reveals it. The
        // `hidden` check is anchored to the banner's own attributes so it can't pass
        // on an unrelated `aria-hidden` elsewhere on the page.
        assert!(html.contains(r#"id="conn-banner""#));
        assert!(html.contains(r#"role="status""#));
        assert!(html.contains(r#"role="status" aria-live="polite" hidden"#));
        // Copy lives in data attributes so it is locked here and read by app.js.
        assert!(html.contains(r#"data-msg-offline="You're offline. Changes can't be saved.""#));
        assert!(html.contains(r#"data-msg-retrying="Lost connection. Retrying…""#));
        assert!(
            html.contains(
                r#"data-msg-failed="Couldn't reach the server. Your change wasn't saved.""#
            )
        );
        assert!(html.contains(
            r#"data-msg-failed-rescan="Couldn't reach the server. The library wasn't rescanned.""#
        ));
        assert!(html.contains(r#"data-msg-reconnected="Reconnected.""#));
        // The message slot the JS fills.
        assert!(html.contains(r#"class="conn-banner-msg""#));
    }

    #[test]
    fn index_renders_the_confirm_dialog() {
        let html = confirm_dialog().into_string();
        // A single page-level dialog the confirm flow fills and opens.
        assert!(html.contains(r#"id="confirm-mark""#));
        assert!(html.contains("Don't ask again"));
        assert!(html.contains(r#"id="confirm-accept""#));
        assert!(html.contains(r#"id="confirm-cancel""#));
    }

    #[test]
    fn index_renders_the_toast_stack_and_template() {
        let html = toast().into_string();
        // An empty stack container plus a template the script clones per toast, so
        // up to three coexist and survive the htmx section swaps.
        assert!(html.contains(r#"id="toast-stack""#));
        assert!(html.contains(r#"id="toast-template""#));
        // The template toast carries the undo button and the message slot.
        assert!(html.contains("toast-undo"));
        assert!(html.contains(r#"class="toast-msg""#));
    }

    #[test]
    fn index_renders_the_mark_warn_template() {
        // The mark-failure strip's warn glyph is served once from a hidden
        // <template>. app.js clones it per failure (M50). The template id is
        // the wire contract between the markup and the script.
        let html = mark_warn_template().into_string();
        assert!(html.contains(r#"id="mark-warn-tpl""#));
        // The glyph itself rides in: the SVG asset's distinctive viewBox.
        assert!(html.contains("viewBox=\"0 0 24 24\""));
    }

    #[test]
    fn page_renders_a_footer_with_version_and_links() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The footer ships in the core shell, so every deployment gets it.
        assert!(html.contains(r#"<footer class="site-footer">"#));
        // It names the product and stamps the crate version.
        assert!(html.contains("Missing Ebooks"));
        assert!(html.contains(concat!("v", env!("CARGO_PKG_VERSION"))));
        // A path home to the source.
        assert!(html.contains(r#"href="https://github.com/noahbaculi/missing-ebooks""#));
    }

    #[test]
    fn navbar_renders_the_help_button() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // A labelled "?" icon button that re-shows the intro card. It takes its
        // own job rather than the `?` key, which already opens settings.
        assert!(html.contains(r#"id="intro-help""#));
        assert!(html.contains(r#"aria-label="How this works""#));
        assert!(html.contains(r#"class="btn btn-ghost btn-square intro-help""#));
        // The toggle advertises its target and current state to assistive tech.
        // The initial value matches the default render (card shown). app.js
        // reconciles it against the per-device dismissal flag on
        // DOMContentLoaded and keeps it in sync on every toggle.
        assert!(html.contains(r#"aria-controls="intro-card""#));
        assert!(html.contains(r#"aria-expanded="true""#));
        // It sits beside the settings cog, before it in the navbar.
        let help_at = html.find(r#"id="intro-help""#).unwrap();
        let cog_at = html
            .find(r#"class="btn btn-ghost btn-square settings-cog""#)
            .unwrap();
        assert!(
            help_at < cog_at,
            "the help button should sit just before the settings cog"
        );
    }

    #[test]
    fn page_renders_the_intro_card_above_the_body() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // A dismissible card that orients a cold visitor: it names the concept and
        // the actions, naming the row controls so the tree reads.
        assert!(html.contains(
            r#"<section class="intro-card" id="intro-card" aria-labelledby="intro-title">"#
        ));
        assert!(html.contains(r#"id="intro-dismiss""#));
        // The dismiss button names what it dismisses, since a screen-reader
        // user navigating to it standalone would otherwise hear only "Dismiss".
        assert!(html.contains(r#"aria-label="Dismiss introduction""#));
        // The body names the real row controls so the text maps onto the tree.
        assert!(html.contains("No ebook"));
        assert!(html.contains("Elsewhere"));
        // It sits above the coverage strip: before the stub body in the shell.
        let card_at = html.find(r#"id="intro-card""#).unwrap();
        let body_at = html.find(r#"id="stub""#).unwrap();
        assert!(
            card_at < body_at,
            "the intro card should render above the body"
        );
    }

    #[test]
    fn intro_card_title_carries_the_help_glyph() {
        let html = page(ViewMode::GapsOnly, 0, &stub_body()).into_string();
        // The title is prefixed with the same help-circle glyph as the navbar
        // button that re-shows the card, so the two read as linked. Assert the
        // shape of the inline mark (the `.icon` opener and the surrounding
        // circle that makes it read as a help glyph rather than a different
        // icon) instead of pinning specific path data, so a future SVG tweak
        // that keeps the same intent does not rebreak this test.
        let title_at = html.find(r#"id="intro-title""#).unwrap();
        let title_end = html[title_at..].find("</h2>").unwrap() + title_at;
        let title = &html[title_at..title_end];
        assert!(
            title.contains(r#"<svg class="icon""#),
            "the title should embed an .icon glyph: {title}"
        );
        assert!(
            title.contains("<circle"),
            "the help glyph carries the circling stroke that makes it read as a question mark: {title}"
        );
    }
}
