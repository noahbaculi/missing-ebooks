# Missing Ebooks

A self-hosted tool that scans audiobook library trees and surfaces folders that
hold audio but no ebook, so the gaps can be found and filled.

## Language

**Library root**:
A top-level directory that gets scanned and rendered as its own tree. There can
be more than one. Every folder the tool reports belongs to exactly one root.
_Avoid_: library, collection.

**Node**:
Any folder shown in a rendered tree, whether a flagged folder or a container.
Every node is actionable: in v1 that means writing a marker and following the
search links. A UI exclude action is deferred to a later increment.

**Flagged folder**:
A folder that directly contains at least one audio file and is not covered by an
ebook or marker. These are the gaps the tool exists to surface.
_Avoid_: missing folder, hit, match.

**Container**:
A node that appears only because it holds flagged folders beneath it (for example
an author or series folder with no audio of its own). It is still actionable: in
the reference data, the real exclude and marker targets are author/series
containers, not individual book folders.
_Avoid_: parent, branch, intermediate node.

**Covered**:
A folder is covered when an ebook file or a marker file sits in it or in any
ancestor up to its library root. A covered folder is never flagged. One
ebook/marker covers everything beneath it.
_Avoid_: satisfied, resolved, has-ebook.

**Marker**:
A file whose presence makes a folder covered on purpose. `.no_ebook` means no
ebook exists or could be sourced; `.ebook_elsewhere` means the ebook lives in
another folder.
_Avoid_: flag file, exception file, sentinel.

**Exclude name**:
An exact directory name (case-insensitive) that drops any matching folder and its
descendants from results, anywhere in the tree. In v1 it is hand-edited in config
and applied at load, like an exclude glob; the two differ only in match criterion
(exact name vs glob on the relative path). A UI button to append names at runtime
is deferred.
_Avoid_: ignore, blocklist entry.

**Exclude glob**:
A glob pattern matched against a folder's path relative to its library root,
case-insensitively. A match drops that folder and its descendants, the same way
an exclude name does; the two differ only in match criterion. Glob syntax is
standard; the subtree-dropping follows the gitignore convention for applying
globs to a tree.
_Avoid_: filter, ignore pattern.
