//! Web-agnostic service layer: typed operations (current view, marker write)
//! shared by the HTML UI and a future JSON API. Built in a later increment. It
//! will own the per-root pipeline (canonicalize the root, scan it, build the
//! forest) that the interim CLI inlines today.
