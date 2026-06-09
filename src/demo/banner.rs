//! The demo banner and the single-pass splice that drops it into the app's HTML.
//! The core app renders a bare `<body>` (see src/web.rs), so the banner is
//! inserted immediately after that tag. Injection is idempotent: a page that
//! already carries the banner marker is returned unchanged. The `/mark` partial
//! is a body-less fragment, so it is returned untouched.

use crate::service::ViewMode;

/// A marker class used both to style the banner and to detect a page that has
/// already been injected.
const BANNER_MARKER: &str = "me-demo-banner";

/// The banner markup plus a scoped inline style and the Reset control. Kept
/// self-contained so it does not depend on the app's stylesheet. The reset form
/// carries the current view so the redirect after a reset lands the visitor back
/// where they were.
fn banner_html(mode: ViewMode) -> String {
    format!(
        r#"<div class="me-demo-banner" style="position:sticky;top:0;z-index:9999;display:flex;align-items:center;justify-content:center;gap:12px;background:#1f2933;color:#fff;font:14px/1.4 system-ui,sans-serif;padding:8px 12px;text-align:center"><span>Demo sandbox with sample data. Your changes are private and reset when idle.</span><form method="post" action="/reset" style="margin:0"><input type="hidden" name="view" value="{view}"><button type="submit" style="cursor:pointer;border:1px solid #fff;background:transparent;color:#fff;font:inherit;border-radius:6px;padding:2px 10px">Reset</button></form></div>"#,
        view = mode.as_query()
    )
}

/// Splice the banner in just after the opening `<body>` tag. Returns the input
/// untouched when there is no `<body>` or the banner is already present.
pub fn inject(html: &str, mode: ViewMode) -> String {
    if html.contains(BANNER_MARKER) {
        return html.to_string();
    }
    // This literal couples the splice to the bare `<body>` that web.rs renders.
    // If that tag ever gains a class or attribute the find misses and the banner
    // silently drops out; the_index_carries_the_demo_banner_but_the_partial_does_not
    // in handlers.rs is the test that fails when that happens.
    let banner = banner_html(mode);
    match html.find("<body>") {
        Some(idx) => {
            let cut = idx + "<body>".len();
            let mut out = String::with_capacity(html.len() + banner.len());
            out.push_str(&html[..cut]);
            out.push_str(&banner);
            out.push_str(&html[cut..]);
            out
        }
        None => html.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_once_after_body() {
        let page = "<html><body><nav>hi</nav></body></html>";
        let out = inject(page, ViewMode::GapsOnly);
        assert!(out.contains(BANNER_MARKER));
        assert!(out.find("<body>").unwrap() < out.find(BANNER_MARKER).unwrap());
        assert_eq!(out.matches(BANNER_MARKER).count(), 1);
    }

    #[test]
    fn is_idempotent() {
        let once = inject("<html><body>x</body></html>", ViewMode::GapsOnly);
        let twice = inject(&once, ViewMode::GapsOnly);
        assert_eq!(once, twice);
    }

    #[test]
    fn leaves_bodyless_html_unchanged() {
        let fragment = "<div class=\"row\">partial swap</div>";
        assert_eq!(inject(fragment, ViewMode::GapsOnly), fragment);
    }

    #[test]
    fn carries_the_reset_form_with_the_current_view() {
        let out = inject("<html><body>x</body></html>", ViewMode::All);
        // The banner now holds a reset form that posts the current view.
        assert!(out.contains(r#"action="/reset""#));
        assert!(out.contains(r#"name="view" value="all""#));
        // The original notice text survives.
        assert!(out.contains("Your changes are private"));
    }

    #[test]
    fn the_reset_form_carries_the_gaps_view() {
        let out = inject("<html><body>x</body></html>", ViewMode::GapsOnly);
        assert!(out.contains(r#"name="view" value="gaps""#));
    }
}
