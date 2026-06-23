//! The three embedded static assets and their conditional-GET serving. Each asset
//! is an `Asset`: its bytes, content type, cache lifetime, and a content-hashed
//! ETag filled once on first request. The bytes are embedded with `include_str!`
//! so the binary carries its own copy.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::OnceLock;

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

/// One servable asset: its bytes, the headers it answers with, and a lazily
/// computed ETag. Held in a `static`, so `respond` borrows `&'static self`.
struct Asset {
    body: &'static str,
    content_type: &'static str,
    cache_control: &'static str,
    etag: OnceLock<String>,
}

impl Asset {
    /// Serve the asset with revalidation. A matching `If-None-Match` gets a `304`
    /// with `ETag` and `Cache-Control` and no body; any other request gets a `200`
    /// with the body and all three headers. The ETag is computed once.
    fn respond(&'static self, headers: &HeaderMap) -> Response {
        let etag = self.etag.get_or_init(|| asset_etag(self.body));
        let requested = headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok());
        if if_none_match_hit(requested, etag) {
            return (
                StatusCode::NOT_MODIFIED,
                [
                    (header::ETAG, etag.as_str()),
                    (header::CACHE_CONTROL, self.cache_control),
                ],
            )
                .into_response();
        }
        (
            [
                (header::CONTENT_TYPE, self.content_type),
                (header::CACHE_CONTROL, self.cache_control),
                (header::ETAG, etag.as_str()),
            ],
            self.body,
        )
            .into_response()
    }
}

/// The embedded stylesheet and script bytes, named so the colocated
/// `mod tests` can substring-check them without going through the handler.
/// The `STYLES` and `SCRIPT` `Asset` fields below borrow them; the bytes
/// still embed once. No visibility modifier: the `mod tests` child sees
/// private items of its parent module, and nothing outside `assets` needs
/// the const.
const APP_CSS_BYTES: &str = include_str!("../../assets/app.css");
const APP_JS_BYTES: &str = include_str!("../../assets/app.js");

/// Cache lifetimes. htmx is vendored and changes only on a version bump, so a
/// week; the stylesheet and script change often, so an hour. None carry
/// `immutable`: the URLs are not fingerprinted, so the ETag must stay free to
/// revalidate once the window passes.
static HTMX: Asset = Asset {
    body: include_str!("../../assets/htmx.min.js"),
    content_type: "text/javascript;charset=utf-8",
    cache_control: "public, max-age=604800",
    etag: OnceLock::new(),
};
static HTMX_SSE: Asset = Asset {
    body: include_str!("../../assets/htmx-sse.js"),
    content_type: "text/javascript;charset=utf-8",
    cache_control: "public, max-age=604800",
    etag: OnceLock::new(),
};
static STYLES: Asset = Asset {
    body: APP_CSS_BYTES,
    content_type: "text/css;charset=utf-8",
    cache_control: "public, max-age=3600",
    etag: OnceLock::new(),
};
static SCRIPT: Asset = Asset {
    body: APP_JS_BYTES,
    content_type: "text/javascript;charset=utf-8",
    cache_control: "public, max-age=3600",
    etag: OnceLock::new(),
};

pub(crate) async fn htmx_script(headers: HeaderMap) -> Response {
    HTMX.respond(&headers)
}

pub(crate) async fn htmx_sse_script(headers: HeaderMap) -> Response {
    HTMX_SSE.respond(&headers)
}

pub(crate) async fn app_css(headers: HeaderMap) -> Response {
    STYLES.respond(&headers)
}

pub(crate) async fn app_js(headers: HeaderMap) -> Response {
    SCRIPT.respond(&headers)
}

/// A strong ETag for an asset: a quoted hash of its bytes. It depends only on
/// content, so it is fixed for the life of the process and identical across
/// restarts built from the same bytes, and a cached validator survives any
/// redeploy that left the asset unchanged.
fn asset_etag(body: &str) -> String {
    let mut hasher = DefaultHasher::new();
    body.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
}

/// Whether an `If-None-Match` value revalidates against `etag`. A bare `*` matches
/// any representation (RFC 9110 §13.1.2) and the asset always exists, so it
/// revalidates. Otherwise the value is a comma list whose candidates may carry the
/// `W/` weak prefix an edge added. `If-None-Match` uses weak comparison, treating
/// `W/"x"` and `"x"` as equal, so each candidate is trimmed and unwrapped before
/// the compare.
fn if_none_match_hit(value: Option<&str>, etag: &str) -> bool {
    let Some(value) = value else { return false };
    value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
    })
}

#[cfg(test)]
mod tests {
    use super::{APP_CSS_BYTES, APP_JS_BYTES, if_none_match_hit};

    #[test]
    fn no_header_never_revalidates() {
        assert!(!if_none_match_hit(None, "\"abc\""));
    }

    #[test]
    fn star_always_revalidates() {
        assert!(if_none_match_hit(Some("*"), "\"abc\""));
    }

    #[test]
    fn exact_match_revalidates() {
        assert!(if_none_match_hit(Some("\"abc\""), "\"abc\""));
    }

    #[test]
    fn weak_prefix_is_unwrapped_before_compare() {
        assert!(if_none_match_hit(Some("W/\"abc\""), "\"abc\""));
    }

    #[test]
    fn one_match_in_a_comma_list_revalidates() {
        assert!(if_none_match_hit(Some("\"x\", W/\"abc\""), "\"abc\""));
    }

    #[test]
    fn a_different_tag_misses() {
        assert!(!if_none_match_hit(Some("\"other\""), "\"abc\""));
    }

    #[test]
    fn app_script_collapses_the_leaving_row() {
        // Before a gaps-only mark request goes out, the script collapses the leaving
        // row so the rows below glide up; the delayed htmx swap reconciles after.
        assert!(APP_JS_BYTES.contains("htmx:beforeRequest"));
        assert!(APP_JS_BYTES.contains("leaving"));
        // collapseRow walks up from the marked leaf and collapses each ancestor that is
        // the sole `:scope > li` in its list, so an emptied author or series row leaves
        // with the leaf instead of snapping out on the swap.
        assert!(APP_JS_BYTES.contains(":scope > li"));
    }

    #[test]
    fn app_script_blurs_the_mark_button_before_the_swap() {
        // The section swap removes the focused mark button. Left focused, the browser
        // jumps the scroll to the page bottom (true in both views), so the script
        // drops focus before the swap.
        assert!(APP_JS_BYTES.contains("blur"));
    }

    #[test]
    fn stylesheet_collapses_the_leaving_row_and_respects_reduced_motion() {
        // A leaving row collapses its height and fades; motion-sensitive users get the
        // instant removal instead.
        assert!(APP_CSS_BYTES.contains(".leaving"));
        assert!(APP_CSS_BYTES.contains("max-height"));
        assert!(APP_CSS_BYTES.contains("prefers-reduced-motion"));
    }

    #[test]
    fn stylesheet_styles_container_depth() {
        // Each depth rule has its own guard so the two switches toggle them
        // independently; the default leaves both attributes absent.
        assert!(APP_CSS_BYTES.contains(r#"html:not([data-bold-top="off"]) .container-top .name"#));
        assert!(
            APP_CSS_BYTES
                .contains(r#"html:not([data-italic-nested="off"]) .container-nested .name"#)
        );
        assert!(APP_CSS_BYTES.contains("font-style: italic"));
    }

    #[test]
    fn stylesheet_styles_the_noscript_notice() {
        assert!(APP_CSS_BYTES.contains(".noscript-notice"));
    }

    #[test]
    fn stylesheet_carries_the_scan_bar_indeterminate() {
        // The rescan indicator is a slim indeterminate bar; the skeleton shimmer is gone.
        assert!(APP_CSS_BYTES.contains(".scan-bar"));
        assert!(APP_CSS_BYTES.contains("@keyframes scan-indeterminate"));
        assert!(!APP_CSS_BYTES.contains(".scan-skeleton"));
        assert!(!APP_CSS_BYTES.contains("@keyframes shimmer"));
        // htmx-request is what reveals the bar; without this rule the indicator would
        // stay invisible for the whole scan, so guard the show rule itself.
        assert!(APP_CSS_BYTES.contains(".scan-bar.htmx-request"));
        // The bar pins to the positioned wrapper, and the tree dims in place rather than
        // being hidden, so the user keeps their spot.
        assert!(APP_CSS_BYTES.contains(".roots-wrap"));
        assert!(APP_CSS_BYTES.contains(".roots-wrap:has(.scan-bar.htmx-request) #roots"));
        // The rescan button dims and shows a locked cursor while disabled.
        assert!(APP_CSS_BYTES.contains("#rescan-btn:disabled"));
    }

    #[test]
    fn stylesheet_carries_the_mobile_layout_rules() {
        assert!(APP_CSS_BYTES.contains("@media (max-width: 600px)"));
        assert!(APP_CSS_BYTES.contains(".actions-trigger"));
        // The action group is now a bottom sheet driven by the popover API, not
        // the old `data-actions-open` toggle.
        assert!(APP_CSS_BYTES.contains(":popover-open"));
        assert!(APP_CSS_BYTES.contains("::backdrop"));
        assert!(!APP_CSS_BYTES.contains("data-actions-open"));
    }

    #[test]
    fn stylesheet_lays_out_marker_tiles_side_by_side() {
        // The two marker buttons share a row as equal-width tiles.
        assert!(APP_CSS_BYTES.contains(".actions-group .mark .btn"));
        assert!(APP_CSS_BYTES.contains("flex-direction: row"));
    }

    #[test]
    fn stylesheet_left_aligns_the_sheet_search_links() {
        // The links column stretches to full width instead of centering, so the
        // search links share the marker buttons' left edge.
        assert!(APP_CSS_BYTES.contains("align-items: stretch"));
    }

    #[test]
    fn stylesheet_collapses_the_flagged_badge_and_keeps_rows_on_one_line() {
        // On mobile the "needs ebook" pill collapses to an amber dot. The label is
        // pushed out of the box with the image-replacement idiom (text-indent), not
        // removed, so it stays in the HTML for screen readers.
        assert!(APP_CSS_BYTES.contains("text-indent: 100%"));
        // Non-covered rows stop wrapping, so the dot and kebab stay on the first
        // line and a long name wraps inside its own box instead.
        assert!(APP_CSS_BYTES.contains(".row:not(.covered)"));
        assert!(APP_CSS_BYTES.contains("overflow-wrap: anywhere"));
    }

    #[test]
    fn stylesheet_stacks_the_navbar_view_toggle_into_a_full_width_row() {
        // The segmented view toggle drops to its own row at full width, with the
        // two segments sharing it as equal-width halves. A child combinator scopes
        // the rule to the navbar's own control, so the settings panel's nested
        // theme segmented control can't inherit the full-width row layout.
        assert!(APP_CSS_BYTES.contains(".navbar > .segmented"));
        assert!(APP_CSS_BYTES.contains("flex-basis: 100%"));
        assert!(APP_CSS_BYTES.contains(".navbar > .segmented .segment"));
        // The settings cog is ordered by a dedicated class, not a `> button`
        // child selector, so a later navbar button can't drift into its row.
        assert!(APP_CSS_BYTES.contains(".navbar .settings-cog"));
        assert!(!APP_CSS_BYTES.contains(".navbar > button"));
    }

    #[test]
    fn stylesheet_indents_the_mobile_cover_files_past_the_name() {
        // Covering filenames drop below the folder name and indent past where the
        // name starts, so they read as subordinate rather than lining up flush.
        assert!(APP_CSS_BYTES.contains(".cover-files"));
        assert!(APP_CSS_BYTES.contains("padding-left: 3.5rem"));
    }

    #[test]
    fn app_script_defines_the_theme_setter() {
        assert!(APP_JS_BYTES.contains("setTheme"));
        assert!(APP_JS_BYTES.contains("confirmMarks"));
        // The two depth toggles are wired through a shared setter and their keys.
        assert!(APP_JS_BYTES.contains("setStylePref"));
        assert!(APP_JS_BYTES.contains("boldTopFolder"));
        assert!(APP_JS_BYTES.contains("italicNestedFolders"));
    }

    #[test]
    fn app_script_defines_the_accent_applier() {
        // The derivation helper, the inline ink token it sets, and the key.
        assert!(APP_JS_BYTES.contains("deriveWarningInk"));
        assert!(APP_JS_BYTES.contains("--color-warning-text"));
        assert!(APP_JS_BYTES.contains(r#""accent""#));
        // The applier and the persist-and-apply entry point.
        assert!(APP_JS_BYTES.contains("applyAccent"));
        assert!(APP_JS_BYTES.contains("setAccent"));
    }

    #[test]
    fn stylesheet_styles_the_confirm_dialog() {
        // The dialog is themed and dims the page behind it.
        assert!(APP_CSS_BYTES.contains(".confirm-dialog"));
        assert!(APP_CSS_BYTES.contains(".confirm-dialog::backdrop"));
        // The non-matching marker glyph hides via the `hidden` attribute. The
        // explicit `.confirm-icon` display must honor it, or both glyphs show.
        assert!(APP_CSS_BYTES.contains(".confirm-icon[hidden]"));
    }

    #[test]
    fn stylesheet_styles_the_toast_and_its_variants() {
        // The stack container and the toast box.
        assert!(APP_CSS_BYTES.contains(".toast-stack"));
        assert!(APP_CSS_BYTES.contains(".toast"));
        // The success toast reveals its glyph in a tinted status badge.
        assert!(APP_CSS_BYTES.contains(".toast--success .toast-icon-success"));
        // The badge glyph resets the muted `.icon` color so it takes the variant
        // color; without this the glyph renders grey.
        assert!(APP_CSS_BYTES.contains(".toast .toast-icon .icon"));
        // The two-line message: the folder name over the outcome and label pill.
        assert!(APP_CSS_BYTES.contains(".toast-name"));
        assert!(APP_CSS_BYTES.contains(".toast-detail"));
        assert!(APP_CSS_BYTES.contains(".toast-kind"));
        // The arrival animation.
        assert!(APP_CSS_BYTES.contains("@keyframes toast-in"));
        // The arrival: a soft `ease` curve over 380ms, sliding the 1.4rem distance.
        // Brisk enough to register beside the row collapse, still gentle.
        assert!(APP_CSS_BYTES.contains("toast-in 380ms ease"));
        assert!(APP_CSS_BYTES.contains("translateY(1.4rem)"));
        // The matching slower exit.
        assert!(APP_CSS_BYTES.contains("toast-out 480ms ease-in"));
        // A settled toast drops its filled entry animation so the script can
        // slide it to a new position when another toast pushes in.
        assert!(APP_CSS_BYTES.contains(".toast--settled"));
    }

    #[test]
    fn stylesheet_styles_the_settings_panel_and_switch() {
        // The settings popover, the switch, and the mobile bottom-sheet form.
        assert!(APP_CSS_BYTES.contains(".settings-panel"));
        assert!(APP_CSS_BYTES.contains(".switch-track"));
        assert!(APP_CSS_BYTES.contains(".settings-panel:popover-open"));
        // The panel opens as a centered overlay (so the cog and the ? hotkey land it
        // in the same place) and dims the page behind it with a backdrop scrim.
        assert!(APP_CSS_BYTES.contains(".settings-panel::backdrop"));
        // The shortcuts reference is styled inside the panel and hidden on mobile.
        assert!(APP_CSS_BYTES.contains(".settings-shortcuts"));
        // Sections are separated by whitespace, not rules: every section start
        // after the first gets a top gap.
        assert!(APP_CSS_BYTES.contains(".settings-head:not(:first-child)"));
    }

    #[test]
    fn stylesheet_styles_the_accent_control() {
        // The quick-pick dots, the active ring, and the native swatch.
        assert!(APP_CSS_BYTES.contains(".accent-dot"));
        assert!(APP_CSS_BYTES.contains(".accent-dot-active"));
        assert!(APP_CSS_BYTES.contains(".accent-swatch"));
    }

    #[test]
    fn stylesheet_defines_the_border_token() {
        // Borders use a dedicated token, lighter than the surface in dark, instead
        // of --color-base-300 (which in dark is darker than the surface and vanishes).
        assert!(APP_CSS_BYTES.contains("--color-border"));
    }

    #[test]
    fn stylesheet_neutralizes_native_button_chrome_on_segments() {
        // The theme segments render as <button>, the view toggle as <span>/<a>.
        // Without an appearance reset the buttons inherit native control chrome
        // (grey fills, beveled borders) and diverge from the flat toggle, so
        // .segment drops it.
        assert!(APP_CSS_BYTES.contains(".segment"));
        assert!(APP_CSS_BYTES.contains("appearance: none"));
    }

    #[test]
    fn app_script_intercepts_marker_writes() {
        assert!(APP_JS_BYTES.contains("htmx:confirm"));
    }

    #[test]
    fn app_script_defines_the_toast_handlers() {
        // The success listener and the undo POST to /unmark.
        assert!(APP_JS_BYTES.contains(r#"addEventListener("marked""#));
        assert!(APP_JS_BYTES.contains("/unmark"));
        // The script drives a stack container and clones a per-toast template.
        assert!(APP_JS_BYTES.contains("toast-stack"));
        assert!(APP_JS_BYTES.contains("toast-template"));
        // The exit-removal delay is a named constant kept in step with the CSS
        // `toast-out` duration.
        assert!(APP_JS_BYTES.contains("EXIT_MS"));
        // The auto-dismiss pauses while the toast is hovered or keyboard-focused.
        assert!(APP_JS_BYTES.contains(r#"addEventListener("mouseenter""#));
        assert!(APP_JS_BYTES.contains(r#"addEventListener("focusin""#));
        // Adding a toast slides the existing ones to their new spot (FLIP, via
        // getBoundingClientRect) over a shared reflow duration rather than
        // letting them jump.
        assert!(APP_JS_BYTES.contains("REFLOW_MS"));
        assert!(APP_JS_BYTES.contains("getBoundingClientRect"));
    }

    #[test]
    fn app_script_toggles_the_summary_end_state() {
        // The recompute shows the all-clear line and hides the hero-and-bar head once
        // the live total reaches zero, and reverses it when an undo brings a gap back,
        // so the live strip lands on the same end-state a reload would render.
        assert!(APP_JS_BYTES.contains(r#"getElementById("gap-summary-clear")"#));
        assert!(APP_JS_BYTES.contains(r#"getElementById("gap-summary-head")"#));
        assert!(APP_JS_BYTES.contains("clear.hidden = total !== 0"));
        assert!(APP_JS_BYTES.contains("head.hidden = total === 0"));
        // The live count excludes a mid-collapse row by its collapsing ancestor
        // (`.leaving` rides the <li>, not the flagged row), so the flip leads the
        // delayed swap instead of lagging a beat behind it.
        assert!(APP_JS_BYTES.contains("countGapRows"));
        assert!(APP_JS_BYTES.contains(r#"closest(".leaving")"#));
    }

    #[test]
    fn stylesheet_styles_the_gap_summary_and_coverage_bar() {
        // The strip, its chips, and the coverage bar are themed.
        assert!(APP_CSS_BYTES.contains(".gap-summary"));
        assert!(APP_CSS_BYTES.contains(".gap-chip"));
        assert!(APP_CSS_BYTES.contains(".gap-bar-fill"));
        // The fill animates its width, and the strip stacks on a phone.
        assert!(APP_CSS_BYTES.contains("transition: width"));
        assert!(APP_CSS_BYTES.contains(".gap-summary-head"));
        // The library coverage block is themed.
        assert!(APP_CSS_BYTES.contains(".gap-coverage"));
        // The old session selectors are gone.
        assert!(!APP_CSS_BYTES.contains(".gap-session"));
    }

    #[test]
    fn stylesheet_styles_the_filter_input_and_disabled_state() {
        // The filter input is styled, and the disabled load-window state mutes the
        // whole control until app.js enables it.
        assert!(APP_CSS_BYTES.contains(".search-input"));
        assert!(APP_CSS_BYTES.contains(".search:has(.search-input:disabled)"));
        assert!(APP_CSS_BYTES.contains(".search-input:disabled"));
        // Filtered-out branches collapse, and the input drops to its own navbar row on
        // a phone, the way the view toggle already reflows.
        assert!(APP_CSS_BYTES.contains(".filter-hidden"));
        assert!(APP_CSS_BYTES.contains(".navbar .search"));
    }

    #[test]
    fn stylesheet_themes_the_clear_button_and_hides_the_native_one() {
        // A themed clear button replaces the browser's native cancel control,
        // hidden here.
        assert!(APP_CSS_BYTES.contains("::-webkit-search-cancel-button"));
        assert!(APP_CSS_BYTES.contains(".search-clear"));
        // It darkens on hover and, while the box is empty, stays in the layout but
        // invisible so its slot is reserved and the field width never changes.
        assert!(APP_CSS_BYTES.contains(".search-clear:hover"));
        assert!(APP_CSS_BYTES.contains(".search-clear[hidden]"));
    }

    #[test]
    fn stylesheet_styles_the_title_home_link() {
        // The title link carries the brand-mark spacing, underlines on hover, and
        // shows a focus ring for keyboard users.
        assert!(APP_CSS_BYTES.contains(".navbar h1 a"));
        assert!(APP_CSS_BYTES.contains(".navbar h1 a:hover"));
        assert!(APP_CSS_BYTES.contains(".navbar h1 a:focus-visible"));
    }

    #[test]
    fn app_script_opens_the_settings_popover_for_the_help_key() {
        // The `?` key opens the merged settings popover; the old cheatsheet helper
        // is gone.
        assert!(APP_JS_BYTES.contains("showPopover"));
        assert!(!APP_JS_BYTES.contains("openCheatsheet"));
    }

    #[test]
    fn app_script_reveals_and_runs_the_filter() {
        // The filter reveals the hidden input, recurses the tree, toggles the
        // collapse class on non-matching branches, and shows the "no matches" line.
        assert!(APP_JS_BYTES.contains("filterTree"));
        assert!(APP_JS_BYTES.contains("filter-hidden"));
        assert!(APP_JS_BYTES.contains("search-empty"));
        assert!(APP_JS_BYTES.contains("clearFilter"));
        // Enter in the box drops focus, so the live filter stays but the keyboard
        // returns to navigation.
        assert!(APP_JS_BYTES.contains(r#"evt.key === "Enter""#));
        // The live filter rides the view-toggle link as a q param and is re-applied
        // from the URL on the next page, so switching views keeps the filter.
        assert!(APP_JS_BYTES.contains("syncViewLink"));
        assert!(APP_JS_BYTES.contains("URLSearchParams"));
    }

    #[test]
    fn app_script_toggles_and_handles_the_clear_button() {
        // The script finds the clear button and toggles its visibility from the input
        // value, so it shows only when the box holds text.
        assert!(APP_JS_BYTES.contains("search-clear"));
        assert!(APP_JS_BYTES.contains("toggleClear"));
    }

    #[test]
    fn app_script_recomputes_the_summary_and_library_coverage() {
        // The summary is recomputed from the DOM as marks land, and the library
        // coverage block reads `data-total-audiobooks` off each section.
        assert!(APP_JS_BYTES.contains("recomputeSummary"));
        assert!(APP_JS_BYTES.contains("updateLibraryCoverage"));
        assert!(APP_JS_BYTES.contains("coverage-bar-fill"));
        assert!(APP_JS_BYTES.contains("totalAudiobooks"));
        // It runs on a confirmed mark, on an undo/section swap, and on a rescan.
        assert!(APP_JS_BYTES.contains(r#"addEventListener("marked""#));
        assert!(APP_JS_BYTES.contains("htmx:afterSwap"));
        // The readout numbers track the bar: covered of total audiobooks, plus
        // the percent.
        assert!(APP_JS_BYTES.contains("coverage-covered"));
        assert!(APP_JS_BYTES.contains("coverage-pct"));
        // The old session plumbing is gone.
        assert!(!APP_JS_BYTES.contains("sessionBaseline"));
        assert!(!APP_JS_BYTES.contains("gapsAtLoad"));
    }

    #[test]
    fn app_script_defines_the_hotkeys_and_active_row() {
        // j/k move a focusable highlight through the visible gap rows; r rescans;
        // / focuses the filter; ? opens the settings popover; Escape clears or drops.
        assert!(APP_JS_BYTES.contains("moveHighlight"));
        assert!(APP_JS_BYTES.contains("visibleGapRows"));
        assert!(APP_JS_BYTES.contains("row-active"));
        // The mark hotkeys were removed, so the row-marking helper is gone too.
        assert!(!APP_JS_BYTES.contains("markActiveRow"));
        // Keys are ignored while typing in a field.
        assert!(APP_JS_BYTES.contains("isEditable"));
    }

    #[test]
    fn stylesheet_styles_the_active_row_highlight() {
        // The j/k highlight is a real focus target: a tinted band and a focus ring.
        assert!(APP_CSS_BYTES.contains(".row-active"));
        assert!(APP_CSS_BYTES.contains("outline"));
    }
}
