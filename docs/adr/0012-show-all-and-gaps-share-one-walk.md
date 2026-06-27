# Show-all and gaps-only are reductions over one walk

Date: 2026-06-22.

## Context

This began as two walks. Gaps-only ran a coverage-pruning walk that stopped descending the moment it hit an ebook or marker, show-all ran the full walk, and the two were kept apart to keep the common path off the fuller walk on a large networked library. The benchmark retired that split: coverage in a real library sits at the leaf book folders and prunes no subtree, so the gaps-only walk visited the same directories and entries as the full walk and ran within noise of it (see `benchmarks/README.md` and ADR-0019).

## Decision

Show-all renders the full library tree, covered folders included; gaps-only renders only the folders with audio and no ebook coverage. Both views are reductions over a single scan: `scanner` runs one walk and the two views derive from its output, gaps-only through `reduce_to_flagged` and show-all through `tree::build`. Gaps-only stays the default landing view.

Folding gaps-only into a reduction over the full walk costs nothing there, drops a second walk shape to maintain, and lets the dir index (ADR-0020) serve both views from one set of cached directories.

The model change that enables this: `tree::Node` carried one `flagged: bool`, which cannot express a covered folder. It now carries two facts, `directly_holds_audio` and `missing_ebook`, and the gap is the derived `needs_ebook()`. Gaps-only output is unchanged, because there `needs_ebook()` reproduces the old `flagged` value.

## Consequences

The scanner does not track why a folder is covered, so there is no "ebook here" versus "covered above" annotation on covered rows. ADR-0013 refines the "ebook here" half: cover files are now listed where they physically sit.

The original decision paired this with one cache slot per view mode behind a shared mutex. That mechanism is superseded by ADR-0022: the cache now holds the raw scan output and renders per request, so there are no per-mode slots and no per-mode edit paths on a marker write. The single-walk principle this ADR records still holds; the cache-layer extension lives in ADR-0022, with the change in commit `81909d1`.
