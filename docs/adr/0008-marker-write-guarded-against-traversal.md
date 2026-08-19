# The marker write re-validates its target inside a configured root

Date: 2026-06-06.

## Context

This is the second layer behind ADR-0003. The write endpoint has no authentication, which is the reason the server binds loopback by default.

## Decision

The marker write takes two request fields that identify the target: a root index into the configured `library_roots`, and `rel`, the folder's path relative to that root. The root base is server-side config, so only `rel` comes from the request. Before writing, the server resolves the target and confirms it sits inside the root: it canonicalizes the configured root, joins `rel`, canonicalizes the result, and checks that the canonical target still begins with the canonical root. A target that escapes the root is rejected as `OutsideRoots`, one that does not exist or cannot be canonicalized as `TargetMissing`, and a file rather than a folder as `NotADirectory`. Only a target that clears all three checks gets the marker file written into it.

We canonicalize both paths and compare them, rather than scanning `rel` for `..` segments, because a lexical check misses symlinks. A folder under the root could be a symlink whose real location is elsewhere, and a string check on `rel` would wave it through. Canonicalizing resolves both `..` and symlinks first, so the prefix comparison runs on real filesystem paths. The comparison is `Path::starts_with`, which matches whole path components, so `/lib` does not match `/library`. The scanner already refuses to follow symlinked directories when it walks, so the guard and the scan agree on which folders belong to a root.

## Consequences

The guard means that even a request that reaches `/mark`, whether through a misconfigured bind or an exposed tunnel, cannot create a file outside a library root. The canonicalize calls and the write touch the filesystem, so they run on a blocking task off the async runtime.

## Accepted risks

Three properties of the guard are known and accepted for v1.

The two `canonicalize` calls and the marker `open` are separate syscalls, so a local attacker with write access somewhere under a configured root could swap a symlink between them to redirect the create (a TOCTOU race). Exploiting this requires filesystem write access under a root the operator already trusts. That is outside the threat model in the [security policy](../../.github/SECURITY.md), which assumes the loopback bind or the reverse-proxy front end is the only untrusted edge. The upgrade path, if the threat model changes, is `openat` on an `O_DIRECTORY | O_NOFOLLOW` dirfd followed by `openat(dirfd, filename, O_CREAT | O_EXCL | O_NOFOLLOW)`.

The guard uses lexical `Path::starts_with` on canonical paths and does not compare `st_dev`. Bind mounts and NFS submounts under a configured root are accepted as inside the root. Self-hosted libraries composed from a NAS mount plus a local disk are a first-class use case, and rejecting cross-`dev` targets would break them.

On case-insensitive or Unicode-normalizing filesystems (APFS, SMB, exFAT, HFS+), a `rel` with a distinct byte spelling can resolve to the same canonical file, and the marker lands under the canonical spelling rather than the spelling the operator typed. The target is still inside the configured root, so there is no escape; only a cosmetic mismatch between the request and the on-disk name. The guard does not NFC-normalize path segments.
