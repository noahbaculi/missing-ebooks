//! The demo banner and the single-pass splice that drops it into the app's HTML.
//! The core app renders a bare `<body>` (see src/web.rs), so the banner is
//! inserted immediately after that tag. Injection is idempotent: a page that
//! already carries the banner marker is returned unchanged. The `/mark` partial
//! is a body-less fragment, so it is returned untouched.

use crate::tree::ViewMode;

/// A marker class used both to style the banner and to detect a page that has
/// already been injected.
const BANNER_MARKER: &str = "me-demo-banner";

/// Scoped styles for the banner's one-time sheen sweep. The block lives with the
/// banner, not in app.css, so the bar still renders from the inline styles below
/// if this is dropped. The sweep is off for reduced-motion visitors.
const BANNER_STYLE: &str = r"<style>.me-demo-sheen{position:absolute;top:0;bottom:0;left:0;width:38%;background:linear-gradient(100deg,transparent 0%,rgba(255,255,255,.38) 50%,transparent 100%);transform:translateX(-140%) skewX(-18deg);animation:me-demo-sheen-sweep 1.6s ease-in-out .25s 1 both;pointer-events:none}@keyframes me-demo-sheen-sweep{0%{transform:translateX(-140%) skewX(-18deg)}100%{transform:translateX(360%) skewX(-18deg)}}@media (prefers-reduced-motion:reduce){.me-demo-sheen{animation:none}}</style>";

/// The banner markup, its self-contained styling, and the Reset control. The bar
/// is full-bleed: negative margins cancel the body's 1.5rem padding, with a 1rem
/// gap below before the navbar. Gradient, shadow, and the one-time sheen from
/// [`BANNER_STYLE`] style it without app.css. The bar splits in two: the sandbox
/// notice and the Reset button group on the left, since the notice explains the
/// reset the button performs; the self-host link sits alone on the right and
/// opens GitHub in a new tab. The reset form carries the current view so a reset
/// lands the visitor where they were.
fn banner_html(mode: ViewMode) -> String {
    format!(
        r#"{BANNER_STYLE}<div class="me-demo-banner" style="position:sticky;top:0;z-index:9999;margin:-1.5rem -1.5rem 1rem;display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:14px;overflow:hidden;background:linear-gradient(110deg,#5a57e6 0%,#4f8fd0 100%);color:#fff;font:14px/1.4 system-ui,sans-serif;padding:9px 16px;text-align:center;text-shadow:0 1px 1px rgba(0,0,0,.14);box-shadow:0 4px 12px -7px rgba(0,0,0,.38)"><span class="me-demo-sheen" aria-hidden="true"></span><span style="position:relative;z-index:1;display:inline-flex;align-items:center;gap:11px"><span>Isolated sandbox: changes reset when idle</span><form method="post" action="/reset" style="margin:0"><input type="hidden" name="view" value="{view}"><button type="submit" style="cursor:pointer;border:1px solid rgba(255,255,255,.5);background:transparent;color:#fff;font:inherit;border-radius:6px;padding:3px 10px">Reset</button></form></span><a href="https://github.com/noahbaculi/missing-ebooks#getting-started" target="_blank" rel="noopener noreferrer" style="position:relative;z-index:1;display:inline-flex;align-items:center;gap:6px;flex:none;border:1px solid rgba(255,255,255,.6);background:rgba(255,255,255,.14);color:#fff;font:inherit;font-weight:600;text-decoration:none;border-radius:6px;padding:4px 12px"><svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"></path></svg>Self-host</a></div>"#,
        view = mode.as_query()
    )
}

/// Splice the banner in just after the opening `<body>` tag. Returns the input
/// untouched when there is no `<body>` or the banner is already present.
pub(crate) fn inject(html: &str, mode: ViewMode) -> String {
    if html.contains(BANNER_MARKER) {
        return html.to_string();
    }
    // WARN: couples the splice to the bare `<body>` web.rs renders. If that tag
    // gains a class or attribute, the find misses and the banner drops out.
    // the_index_carries_the_demo_banner_but_the_partial_does_not in handlers.rs is
    // the test that catches it.
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
        // The reset form posts the current view.
        assert!(out.contains(r#"action="/reset""#));
        assert!(out.contains(r#"name="view" value="all""#));
        // The approved sandbox notice replaces the old live-sandbox copy.
        assert!(out.contains("Isolated sandbox: changes reset when idle"));
        assert!(!out.contains("Isolated sandbox: changes reset when idle."));
        assert!(!out.contains("Live sandbox; changes reset when idle."));
        assert!(!out.contains("Changes are private and reset when idle"));
        // The self-host CTA links to the README Getting Started section and opens a new tab.
        assert!(
            out.contains(r#"href="https://github.com/noahbaculi/missing-ebooks#getting-started""#)
        );
        assert!(out.contains(r#"target="_blank""#));
        assert!(out.contains(r#"rel="noopener noreferrer""#));
        // The label is trimmed to "Self-host"; the old "Self-host this" is gone.
        assert!(out.contains("Self-host</a>"));
        assert!(!out.contains("Self-host this"));
        // The sheen markup is spliced in alongside the notice.
        assert!(out.contains("me-demo-sheen"));
    }

    #[test]
    fn the_reset_form_carries_the_gaps_view() {
        let out = inject("<html><body>x</body></html>", ViewMode::GapsOnly);
        assert!(out.contains(r#"name="view" value="gaps""#));
    }
}
