//! The demo banner and the single-pass splice that drops it into the app's HTML.
//! The core app renders a bare `<body>` (see src/web.rs), so the banner is
//! inserted immediately after that tag. Injection is idempotent: a page that
//! already carries the banner marker is returned unchanged. The `/mark` partial
//! is a body-less fragment, so it is returned untouched.

use crate::service::ViewMode;

/// A marker class used both to style the banner and to detect a page that has
/// already been injected.
const BANNER_MARKER: &str = "me-demo-banner";

/// Scoped styles for the banner's one-time sheen sweep. Living in the banner's
/// own `<style>` block rather than app.css keeps the bar self-contained: if this
/// is ever dropped, the gradient and layout still render from the inline styles
/// below. The sweep is switched off for visitors who prefer reduced motion.
const BANNER_STYLE: &str = r#"<style>.me-demo-sheen{position:absolute;top:0;bottom:0;left:0;width:38%;background:linear-gradient(100deg,transparent 0%,rgba(255,255,255,.38) 50%,transparent 100%);transform:translateX(-140%) skewX(-18deg);animation:me-demo-sheen-sweep 1.6s ease-in-out .25s 1 both;pointer-events:none}@keyframes me-demo-sheen-sweep{0%{transform:translateX(-140%) skewX(-18deg)}100%{transform:translateX(360%) skewX(-18deg)}}@media (prefers-reduced-motion:reduce){.me-demo-sheen{animation:none}}</style>"#;

/// The banner markup, its self-contained styling, and the Reset control. The bar
/// is full-bleed: negative margins cancel the body's 1.5rem padding so it sits
/// flush to the top and both edges, with a 1rem gap below before the navbar. A
/// tonal-blue gradient, a soft shadow, and the one-time sheen from [`BANNER_STYLE`]
/// style it without leaning on app.css. The reset form carries the
/// current view so the redirect after a reset lands the visitor where they were.
fn banner_html(mode: ViewMode) -> String {
    format!(
        r#"{BANNER_STYLE}<div class="me-demo-banner" style="position:sticky;top:0;z-index:9999;margin:-1.5rem -1.5rem 1rem;display:flex;align-items:center;justify-content:center;gap:12px;overflow:hidden;background:linear-gradient(110deg,#5a57e6 0%,#4f8fd0 100%);color:#fff;font:14px/1.4 system-ui,sans-serif;padding:9px 16px;text-align:center;text-shadow:0 1px 1px rgba(0,0,0,.14);box-shadow:0 4px 12px -7px rgba(0,0,0,.38)"><span class="me-demo-sheen" aria-hidden="true"></span><span style="position:relative;z-index:1">Demo sandbox with sample data. Changes are private and reset when idle.</span><form method="post" action="/reset" style="margin:0;position:relative;z-index:1"><input type="hidden" name="view" value="{view}"><button type="submit" style="cursor:pointer;border:1px solid rgba(255,255,255,.85);background:transparent;color:#fff;font:inherit;border-radius:6px;padding:2px 10px">Reset</button></form></div>"#,
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
        // The notice text survives the splice.
        assert!(out.contains("Changes are private and reset when idle"));
        // The self-contained sheen markup is spliced in alongside the notice.
        assert!(out.contains("me-demo-sheen"));
    }

    #[test]
    fn the_reset_form_carries_the_gaps_view() {
        let out = inject("<html><body>x</body></html>", ViewMode::GapsOnly);
        assert!(out.contains(r#"name="view" value="gaps""#));
    }
}
