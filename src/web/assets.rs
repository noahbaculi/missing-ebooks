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
static STYLES: Asset = Asset {
    body: include_str!("../../assets/app.css"),
    content_type: "text/css;charset=utf-8",
    cache_control: "public, max-age=3600",
    etag: OnceLock::new(),
};
static SCRIPT: Asset = Asset {
    body: include_str!("../../assets/app.js"),
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
/// content, so it is identical across restarts built from the same bytes, and a
/// cached validator survives any redeploy that left the asset unchanged.
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
    use super::if_none_match_hit;

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
}
