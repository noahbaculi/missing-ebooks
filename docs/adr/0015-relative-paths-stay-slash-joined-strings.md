# Relative paths stay `/`-joined strings, not a newtype

Date: 2026-06-10.

## Context

A folder's path relative to its library root is a `/`-joined string, with `.` reserved for the root itself. The scanner emits a `PathBuf`, the tree builder joins its components with `/`, the tree mutators split on `/`, and the request carries the same string through to the marker write.

## Decision

This convention is left as-is; the path is not wrapped in a `RelPath` newtype.

## Consequences

We considered a newtype owning the format, an `is_root()` method, component iteration, and a `Deserialize` gate at the request edge. We set it aside. The only use that crosses a module seam is the `.`-is-root check (in `service`, `web`, and `tree`); the `/` split and join already live inside `tree.rs` and have locality there. So a newtype would touch every module to relocate a trivial equality check. Its one new capability, rejecting a malformed `rel` at the request edge, guards a case that already no-ops harmlessly when it fails to match a node, and that ADR-0008 independently covers by canonicalizing the marker-write target and confirming it stays inside the root. The cost (a type threaded through `tree`, `service`, `web`, and the demo) clearly outruns the payoff.

If the relative path ever grows real structure, for example a tag-derived query that needs to parse components, or a second writer that needs to validate input the way ADR-0008 validates the target, revisit this: at that point the newtype would concentrate genuine behavior rather than rename a check.
