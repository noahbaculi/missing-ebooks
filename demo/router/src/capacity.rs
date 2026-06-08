//! The page shown when the global sandbox cap is reached. Served with HTTP 503
//! so bots and monitors read it as a soft, retryable refusal.

/// A small self-contained HTML page. No external CSS, since a visitor who hits
/// the cap never reaches a sandbox to load the app's stylesheet.
pub const CAPACITY_HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Missing Ebooks demo is busy</title></head>
<body style="font:16px/1.5 system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem">
<h1>The demo is at capacity</h1>
<p>Every demo sandbox is in use right now. Each one is a throwaway environment
that frees up after a few idle minutes. Please try again shortly.</p>
</body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_is_a_full_html_document() {
        assert!(CAPACITY_HTML.starts_with("<!doctype html>"));
        assert!(CAPACITY_HTML.contains("at capacity"));
    }
}
