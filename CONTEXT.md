# Missing Ebooks

A self-hosted tool that scans audiobook library trees and surfaces folders that hold audio but no ebook, so the gaps can be found and filled.

## Language

**Library root**:
A top-level directory that is scanned and rendered as its own tree. There can be more than one. Every folder the tool reports belongs to exactly one root. A root can itself be flagged: when it directly holds uncovered audio (loose files with no author or book folder beneath it), the root is surfaced as a flagged node at the top of its tree rather than skipped.
_Avoid_: library, collection.

**Node**:
Any folder shown in a rendered tree, whether a flagged folder or a container. Every node is actionable: that means writing a marker and following the search links. A UI exclude action is deferred to a later increment.

**Flagged folder**:
A folder that directly contains at least one audio file and is not covered by an ebook or marker. These are the gaps the tool exists to surface. A library root counts as such a folder: loose, uncovered audio sitting directly in a root flags the root itself.
_Avoid_: missing folder, hit, match.

**Container**:
A folder that directly holds no audio and appears in a tree only because flagged folders sit somewhere beneath it (for example an author or series folder). Holding no direct audio is the defining trait, and it is what separates a container from a flagged folder, so a node is never both at once. A container is still actionable: in the reference data, the real exclude and marker targets are author/series containers, not individual book folders.
_Avoid_: parent, branch, intermediate node.

**Covered**:
A folder is covered when an ebook file or a marker file sits in it or in any ancestor up to its library root. A covered folder is never flagged. One ebook/marker covers everything beneath it.
_Avoid_: satisfied, resolved, has-ebook.

**Directly holds audio / Missing ebook**:
The two independent facts a node carries. _Directly holds audio_ is true when the folder itself contains an audio file. _Missing ebook_ is the inverse of _covered_: true when no ebook or marker sits in the folder or any ancestor. A flagged folder is the pair (directly holds audio, missing ebook); a covered audiobook is (holds audio, not missing); a covered container is (no audio, not missing); a plain container is (no audio, missing) and needs nothing itself.
_Avoid_: has-audio flag, uncovered flag.

**Show-all view**:
An opt-in view that renders the full directory tree, covered folders included, down to the individual book folders. A covered folder shows as a dimmed name with a check, no buttons and no links. The default view stays gaps-only; a toggle switches per view and is not persisted. Marking a folder in show-all turns it from a gap into a covered row in place rather than removing it.
_Avoid_: full view, everything view.

**Marker**:
A file whose presence makes a folder covered on purpose. `.no_ebook` means no ebook exists or could be sourced; `.ebook_elsewhere` means the ebook lives in another folder. Each node row has one button per marker that writes the file into that folder, so marking a container covers every folder beneath it through ancestor coverage.
A just-written marker can be reversed from the undo toast that appears after a mark; undoing deletes that one marker file and rescans its root. Up to three toasts stack at once, so a mark made before the latest one can still be undone; the oldest drops off when a fourth arrives.
_Avoid_: flag file, exception file, sentinel.

**Search link**:
A configured template whose `{query}` placeholder is filled with the folder name, cleaned and percent-encoded, shown on every node row. Following one opens a prefilled book search in a new tab, so the operator can go find the missing ebook without losing the page. The cleaning drops bracketed segments and normalizes separators. The query is built from the folder name only, with a tag-based query deferred.
_Avoid_: lookup, external link.

**Exclude name**:
An exact directory name (case-insensitive) that drops any matching folder and its descendants from results, anywhere in the tree. It is hand-edited in config and applied at load, like an exclude glob; the two differ only in match criterion (exact name vs glob on the relative path). A UI button to append names at runtime is deferred.
_Avoid_: ignore, blocklist entry.

**Exclude glob**:
A glob pattern matched against a folder's path relative to its library root, case-insensitively. A match drops that folder and its descendants, the same way an exclude name does; the two differ only in match criterion. Glob syntax is standard; the subtree-dropping follows the gitignore convention for applying globs to a tree.
_Avoid_: filter, ignore pattern.

**Raw view store**:
The substrate that produces and memoizes raw scan output (`RawViewStore` in `src/state.rs`). Owns the scan settings, the dir index, the TTL-bounded cache slot, and the marker file IO. One slot per process, TTL-bounded by `ttl_seconds`; both view modes render from the same raw data at request time (ADR-0022). Marker writes edit the slot in place (ADR-0002). See ADR-0027.
_Avoid_: render cache, scan cache (ambiguous with the dir index), the cache.

**Dir index**:
The in-memory per-directory mtime map (`dir_index` in `src/state.rs`) that lets a scan skip unchanged directories. Process-lifetime, written by every scan, discarded only by a `/rescan` click or process restart (see ADR-0020).
_Avoid_: cache, mtime cache.

**Warm scan**:
A scan that reuses entries from a populated dir index, checking each directory's mtime via `stat` and re-listing only the ones whose mtime has changed. Fast on a hot index, microseconds per unchanged directory on local storage and the difference between a sub-second and a multi-second walk on SMB.
_Avoid_: incremental scan (the implementation detail), cached scan.

**Cold scan**:
A scan that does not reuse any dir index entries, either because the index is empty (process just started) or because the path explicitly clears it (`/rescan` click). Walks every directory. A cold scan is a `scan_warm` call against a fresh `DirIndex`; there is no separate `scan_cold` function.
_Avoid_: full scan, rescan (the verb for the user action, not the scan type).

**Refresh poll**:
The client-driven refresh path: every open tab hits `GET /refresh` on `poll_interval_seconds` while `document.visibilityState` is `visible`, and swaps the response into `#roots`. The server serves each poll from the cached raw view when it is younger than `ttl_seconds`, so scan rate is capped by TTL regardless of how many tabs are polling. Rescan is the user's explicit "walk from scratch" action; refresh polls are the background pull that keeps a live tab current. See ADR-0034.
_Avoid_: autosync, autorefresh, live update.

**Library coverage**:
The fraction of audiobooks across all successfully-scanned library roots that are covered by an ebook or marker. Numerator and denominator are folder counts: folders that directly hold audio. Errored roots contribute to neither; their failure is already surfaced on the per-root section banner. The reported percentage is `Math.floor(covered / total * 100)`, floored so a single remaining gap never reads as 100%.
_Avoid_: completion, progress, done.
