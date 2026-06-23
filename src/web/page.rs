//! The page shell and the navbar/popover chrome. Imported by `web::render`,
//! never the reverse: see the implementation plan at
//! `docs/superpowers/plans/2026-06-23-b-render-split-and-assets-triage.md`
//! for the call-graph rationale. None of these helpers touch domain types
//! like `Node` or `FlaggedView`; the shell takes the body as a `Markup`
//! parameter so the chrome and the tree never share a module.
