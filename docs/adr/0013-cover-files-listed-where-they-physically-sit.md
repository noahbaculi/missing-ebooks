# Cover files are listed where they physically sit

Date: 2026-06-07.

## Context

This refines ADR-0012, which said the scanner does not track why a folder is covered, so there was no "ebook here" versus "covered above" annotation. The scanner now records the cover files that sit in a folder, which is the "ebook here" half. It still does not resolve "covered above": an ancestor-covered row stays blank, and it reads as marker-covered only because a marker sits locally and shows its name.

## Decision

The show-all view lists, on each covered row, the ebook and marker filenames that physically sit in that folder. A folder covered only by an ancestor's ebook lists nothing of its own; the covering name appears once, on the row that holds the file. `scanner::ScannedFolder` and `tree::Node` carry a `cover_files: Vec<String>` for this, ebooks first then markers, each natural-sorted, collected during the existing `scan_all` walk.

Markers are listed alongside real ebooks, by filename, so a marker-covered folder is distinguishable from an ancestor-covered one. A marker written through the UI in show-all is appended to the row's cover files in memory, so the just-marked row shows it without waiting for a rescan, consistent with the in-place edit of ADR-0002.

## Consequences

We considered resolving the covering file onto every descendant it covers, so each covered row would name its coverer even when the file lives above it. We rejected it. Repeating an ancestor's filename down the subtree adds noise without adding information, because the holding row already shows the name, and resolving coverage at render time reintroduces the ancestor walk the physical model avoids. The operator spot-checks a filename against the folder that holds it, which is exactly the physical case.
