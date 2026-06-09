//! The demo banner and the single-pass splice that drops it into the app's HTML.
//! The core app renders a bare `<body>` (see src/web.rs), so the banner is
//! inserted immediately after that tag. Injection is idempotent: a page that
//! already carries the banner marker is returned unchanged. The `/mark` partial
//! is a body-less fragment, so it is returned untouched.

/// A marker class used both to style the banner and to detect a page that has
/// already been injected.
const BANNER_MARKER: &str = "me-demo-banner";

/// The banner markup plus a scoped inline style. Kept self-contained so it does
/// not depend on the app's stylesheet.
pub const BANNER_HTML: &str = r#"<div class="me-demo-banner" style="position:sticky;top:0;z-index:9999;background:#1f2933;color:#fff;font:14px/1.4 system-ui,sans-serif;padding:8px 12px;text-align:center">Demo sandbox with sample data. Your changes are private and reset when idle.</div>"#;

/// Splice the banner in just after the opening `<body>` tag. Returns the input
/// untouched when there is no `<body>` or the banner is already present.
pub fn inject(html: &str) -> String {
    if html.contains(BANNER_MARKER) {
        return html.to_string();
    }
    match html.find("<body>") {
        Some(idx) => {
            let cut = idx + "<body>".len();
            let mut out = String::with_capacity(html.len() + BANNER_HTML.len());
            out.push_str(&html[..cut]);
            out.push_str(BANNER_HTML);
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
        let out = inject(page);
        assert!(out.contains(BANNER_MARKER));
        assert!(out.find("<body>").unwrap() < out.find(BANNER_MARKER).unwrap());
        assert_eq!(out.matches(BANNER_MARKER).count(), 1);
    }

    #[test]
    fn is_idempotent() {
        let once = inject("<html><body>x</body></html>");
        let twice = inject(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn leaves_bodyless_html_unchanged() {
        let fragment = "<div class=\"row\">partial swap</div>";
        assert_eq!(inject(fragment), fragment);
    }
}
