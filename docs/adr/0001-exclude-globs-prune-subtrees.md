# Exclude-glob matches prune the whole subtree

Date: 2026-06-04.

## Context

Exclude globs are matched against each folder's path relative to its library root.

## Decision

When a glob matches a folder, that folder and its entire subtree are dropped from the scan, identical to how an exclude name behaves. The two exclusion mechanisms then differ only in match criterion (exact directory name anywhere vs. glob on the relative path) and editability (names are UI-editable, globs are hand-edited only).

## Consequences

We considered treating a glob as a strict per-folder predicate, which is the "pure" reading of a glob as a string matcher. We rejected it: this feature applies globs to a directory tree, and the convention there (gitignore, ripgrep, fd, rsync) is that a directory match prunes its contents. We confirmed with globset that `**/*(abridged)*` and `**/*(abridged)*/**` match disjoint sets (the folder vs. its contents, never both), so a per-folder predicate would force paired patterns and would still flag audio-bearing disc subfolders under an excluded book. Pruning on match reproduces the reference script's effective behavior with a single pattern. Glob syntax itself is unchanged. Only the tree-application convention is chosen.
