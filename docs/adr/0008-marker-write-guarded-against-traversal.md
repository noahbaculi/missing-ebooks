# The marker write re-validates its target inside a configured root

Date: 2026-06-06.

## Context

This is the second layer behind ADR-0003. The write endpoint has no authentication, which is the reason the server binds loopback by default.

## Decision

The marker write takes two request fields that identify the target: a root index into the configured `library_roots`, and `rel`, the folder's path relative to that root. The root base is server-side config, so only `rel` comes from the request. Before writing, the server resolves the target and confirms it sits inside the root: it canonicalizes the configured root, joins `rel`, canonicalizes the result, and checks that the canonical target still begins with the canonical root. A target that escapes the root is rejected as `OutsideRoots`, one that does not exist or cannot be canonicalized as `TargetMissing`, and a file rather than a folder as `NotADirectory`. Only a target that clears all three checks gets the marker file written into it.

We canonicalize both paths and compare them, rather than scanning `rel` for `..` segments, because a lexical check misses symlinks. A folder under the root could be a symlink whose real location is elsewhere, and a string check on `rel` would wave it through. Canonicalizing resolves both `..` and symlinks first, so the prefix comparison runs on real filesystem paths. The comparison is `Path::starts_with`, which matches whole path components, so `/lib` does not match `/library`. The scanner already refuses to follow symlinked directories when it walks, so the guard and the scan agree on which folders belong to a root.

## Consequences

The guard means that even a request that reaches `/mark`, whether through a misconfigured bind or an exposed tunnel, cannot create a file outside a library root. The canonicalize calls and the write touch the filesystem, so they run on a blocking task off the async runtime.
