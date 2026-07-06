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
    /// with `ETag` and `Cache-Control` and no body. Any other request gets a `200`
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
/// The `STYLES` and `SCRIPT` `Asset` fields below borrow them. The bytes
/// still embed once. No visibility modifier: the `mod tests` child sees
/// private items of its parent module, and nothing outside `assets` needs
/// the const.
const APP_CSS_BYTES: &str = include_str!("../../assets/app.css");
const APP_JS_BYTES: &str = include_str!("../../assets/app.js");

/// Cache lifetimes. htmx is vendored and changes only on a version bump, so a
/// week. The stylesheet and script change often, so an hour. None carry
/// `immutable`: the URLs are not fingerprinted, so the ETag must stay free to
/// revalidate once the window passes.
static HTMX: Asset = Asset {
    body: include_str!("../../assets/htmx.min.js"),
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
    //! Substring assertions that only mirror a CSS class name or a JS function
    //! name with no cross-file consumer pin tests to the implementation, not to
    //! behavior. Pin behavior with a rendered-HTML test against the router
    //! output instead.
    //!
    //! The `if_none_match_hit` block at the top tests handler logic, not asset
    //! bytes. Those tests are out of triage scope.

    use super::{APP_CSS_BYTES, APP_JS_BYTES, if_none_match_hit};

    const PREPAINT_JS_BYTES: &str = include_str!("../../assets/prepaint.js");

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
    fn app_script_uses_the_depth_storage_keys_from_prepaint() {
        // The two depth-typography opt-outs ride localStorage keys that
        // app.js writes and prepaint.js reads before first paint.
        for key in ["boldTopFolder", "italicNestedFolders"] {
            assert!(APP_JS_BYTES.contains(key), "app.js missing {key}");
            assert!(PREPAINT_JS_BYTES.contains(key), "prepaint.js missing {key}");
        }
    }

    #[test]
    fn app_script_uses_the_accent_storage_key_and_warning_text_token() {
        // The accent picker's storage key and the CSS custom property both
        // scripts set are the wire contract between app.js and prepaint.js.
        // app.js uses double-quoted string literals, prepaint.js uses single.
        assert!(APP_JS_BYTES.contains(r#""accent""#));
        assert!(PREPAINT_JS_BYTES.contains("'accent'"));
        assert!(APP_JS_BYTES.contains("--color-warning-text"));
        assert!(PREPAINT_JS_BYTES.contains("--color-warning-text"));
    }

    #[test]
    fn app_script_intercepts_marker_writes() {
        // `htmx:confirm` is the htmx event app.js binds against to gate
        // marker writes. htmx fires it from the rendered `hx-confirm`
        // attribute on the marker buttons.
        assert!(APP_JS_BYTES.contains("htmx:confirm"));
    }

    #[test]
    fn app_script_listens_for_the_marked_trigger_and_posts_to_unmark() {
        // The HX-Trigger value built by `marked_trigger` in src/web.rs and
        // the `/unmark` route both cross the JS/Rust boundary.
        assert!(APP_JS_BYTES.contains(r#"addEventListener("marked""#));
        assert!(APP_JS_BYTES.contains("/unmark"));
        // The toast-undo swap must target the root's `section.root`, not any
        // element carrying `data-root`. Without the tag qualifier the bare
        // selector hits a gap-summary chip first and the section markup swaps
        // in for the chip, duplicating row state and throwing the live
        // gap-hero count off until a reload.
        assert!(APP_JS_BYTES.contains(r#"target: 'section.root[data-root="'"#));
        assert!(!APP_JS_BYTES.contains(r#"target: '[data-root="'"#));
    }

    #[test]
    fn app_assets_animate_the_undo_restore() {
        // Undo's restore mirrors the mark departure: app.css carries `li.entering`
        // (the inverse of `li.leaving`) and app.js runs `expandRow` off the
        // `/unmark` afterSwap to slide the restored spine back into the gaps list.
        assert!(APP_CSS_BYTES.contains("li.entering"));
        assert!(APP_JS_BYTES.contains("expandRow"));
    }

    #[test]
    fn app_script_wires_the_online_event_to_flash_or_hide() {
        // The window `online` handler reads `bannerShowsProblem()` (which
        // checks the three connection-banner states) and calls
        // `flashReconnected` when one is set, `hideBanner` when the banner is
        // clean. No DOM harness exists for app.js, so pin the wire shape as a
        // substring check: the handler must be there, must branch on
        // `bannerShowsProblem`, and must call both branches by name.
        assert!(APP_JS_BYTES.contains(r#"addEventListener("online""#));
        assert!(APP_JS_BYTES.contains("bannerShowsProblem()"));
        assert!(APP_JS_BYTES.contains("flashReconnected()"));
        assert!(APP_JS_BYTES.contains("hideBanner"));
        // `bannerShowsProblem` itself reads the three problem-state classes.
        for state in ["is-offline", "is-retrying", "is-failed"] {
            assert!(
                APP_JS_BYTES.contains(state),
                "app.js missing banner state {state}"
            );
        }
    }

    #[test]
    fn app_script_clones_the_mark_warn_template_rather_than_inlining_an_svg() {
        // The mark-failure strip used to hold a duplicate SVG string. The
        // single source of truth is now assets/svg/warning.svg, embedded once
        // in the page shell as <template id="mark-warn-tpl"> and cloned here.
        assert!(APP_JS_BYTES.contains(r#"getElementById("mark-warn-tpl")"#));
        assert!(APP_JS_BYTES.contains(".content.cloneNode("));
        // No inline SVG payload survives.
        assert!(!APP_JS_BYTES.contains("MARK_WARN_SVG"));
        assert!(!APP_JS_BYTES.contains("<svg viewBox=\"0 0 24 24\""));
    }

    #[test]
    fn app_script_reads_the_two_summary_end_state_ids() {
        // `gap-summary-clear` and `gap-summary-head` are DOM ids emitted by
        // `gap_summary` in render.rs and looked up here by id.
        assert!(APP_JS_BYTES.contains(r#"getElementById("gap-summary-clear")"#));
        assert!(APP_JS_BYTES.contains(r#"getElementById("gap-summary-head")"#));
    }

    #[test]
    fn stylesheet_does_not_carry_the_removed_gap_session_class() {
        // The old session selectors are gone. Fence them off.
        assert!(!APP_CSS_BYTES.contains(".gap-session"));
    }

    #[test]
    fn app_script_syncs_help_button_aria_expanded_with_the_intro_flag() {
        // The intro-card help button advertises its toggle state to assistive
        // tech. The reconcile happens at two sites in app.js: the per-toggle
        // write inside `setIntroDismissed`, and the DOMContentLoaded hookup
        // that aligns the markup's default `aria-expanded="true"` with the
        // pre-paint `dataset.intro` flag for a returning visitor. Pin both so
        // a future refactor can't silently drop one half of the contract.
        assert!(APP_JS_BYTES.contains(r#"help.setAttribute("aria-expanded""#));
        assert!(APP_JS_BYTES.contains(r#"helpInit.setAttribute("aria-expanded""#));
        assert!(APP_JS_BYTES.contains(r#"dataset.intro === "dismissed""#));
    }

    #[test]
    fn app_script_uses_the_search_empty_id() {
        // `search-empty` is the DOM id emitted by `search_empty()` in
        // page.rs. The filter reveals it when nothing matches.
        assert!(APP_JS_BYTES.contains("search-empty"));
    }

    #[test]
    fn app_script_uses_the_search_clear_id() {
        // `search-clear` is the DOM id emitted by `search_box()` in page.rs.
        // The script toggles its visibility from the input value.
        assert!(APP_JS_BYTES.contains("search-clear"));
    }

    #[test]
    fn app_script_reads_the_coverage_ids_and_data_total_audiobooks() {
        // The coverage DOM ids are emitted by `coverage_bar` in render.rs.
        // `totalAudiobooks` is the camel-case form of the
        // `data-total-audiobooks` attribute the section markup carries
        // (ADR-0025).
        for id in ["coverage-bar-fill", "coverage-covered", "coverage-pct"] {
            assert!(APP_JS_BYTES.contains(id), "app.js missing {id}");
        }
        assert!(APP_JS_BYTES.contains("totalAudiobooks"));
        // The old session plumbing is gone.
        assert!(!APP_JS_BYTES.contains("sessionBaseline"));
        assert!(!APP_JS_BYTES.contains("gapsAtLoad"));
    }

    #[test]
    fn app_script_does_not_carry_the_removed_cheatsheet() {
        // The merged settings popover replaced the old cheatsheet helper.
        // Fence the helper off so it does not creep back in.
        assert!(!APP_JS_BYTES.contains("openCheatsheet"));
    }
}
