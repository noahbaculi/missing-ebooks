//! Tripwire for the vendored htmx assets. Both files ship without an upstream
//! manifest, so a silent edit (intentional or otherwise) has no other guardrail.
//! When upstream is updated on purpose, also update the provenance header in
//! the asset file and the pinned digest below.

use sha2::{Digest, Sha256};

// Pinned against the file bytes that include the provenance header committed
// alongside this test. Recompute with `sha256sum assets/htmx.min.js` after any
// intentional edit.
const HTMX_MIN_JS_SHA256: &str = "e7dc9320d16ad5a4ebe5204f7cb3a74f11d084cee65fcecdbc51444a737ea522";
const HTMX_SSE_JS_SHA256: &str = "f00bc7acdb9b3045eaeb7548925df70e05c5c1db9d1699b960fbfeaeadb670af";

#[test]
fn htmx_min_js_matches_pinned_digest() {
    let bytes = include_bytes!("../assets/htmx.min.js");
    let actual = format!("{:x}", Sha256::digest(bytes));
    assert_eq!(
        actual, HTMX_MIN_JS_SHA256,
        "vendored htmx.min.js has drifted; update the provenance header and the pinned digest together"
    );
}

#[test]
fn htmx_sse_js_matches_pinned_digest() {
    let bytes = include_bytes!("../assets/htmx-sse.js");
    let actual = format!("{:x}", Sha256::digest(bytes));
    assert_eq!(
        actual, HTMX_SSE_JS_SHA256,
        "vendored htmx-sse.js has drifted; update the provenance header and the pinned digest together"
    );
}
