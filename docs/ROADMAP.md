# Roadmap

Ideas parked for after v1. Nothing here is committed work. Each entry graduates into a `.scratch/<feature>/` issue when it gets picked up.

## Machine-readable API

A JSON or GraphQL endpoint so other tools can read scan results without scraping the HTML. The current router only serves rendered pages, so anything programmatic has to parse markup. Decide JSON vs GraphQL once the query shapes are clearer. JSON is the smaller step.

## Agent skill for the tool

A skill that teaches an agent to drive the tool: trigger a scan, read which folders are missing ebooks, mark and unmark. Depends on the machine-readable API above, since a skill scraping HTML would be as brittle as anything else.

## Tag-based search query

Build the search-link query from embedded audio tags (author, title) read at scan time, instead of from the folder name. Folder names are often messy, so a tag-derived query finds the book more reliably. The `src/query.rs` module comment sketches the shape: read tags during the walk, store the query on the cached node, and skip `clean_query` for the populated case. Tag reads are filesystem work, so they belong at scan time, not per render on a networked mount.

## Tag-based scanning

Identify and group audiobooks by their embedded metadata, the way Audiobookshelf does, rather than inferring everything from folder layout. Today the tool is folder-granular (ADR-0007) and reads no tags: a folder is an audiobook if it directly holds audio. Reading author, title, series, and narrator from the audio files would let coverage and grouping follow the actual books even when the folders are inconsistent or flat. This is the larger sibling of the tag-based search query above and shares its scan-time tag read, but it touches how the scanner decides what a book is, so it is a bigger change than swapping the query source.

## Show active config in the web UI

Surface the loaded config (library roots, `excluded_dirs`, `exclude_globs`, extensions) somewhere in the UI so a user can tell why a folder is absent from the tree without shelling into the container to read TOML. Read-only display, not an edit surface: ADR-0022 keeps config immutable at runtime, and this entry does not reopen that. Motivating case: a subtree pruned by an exclude glob (`(Dramatized Adaptation)` under a series folder) is silently missing today, with no in-UI signal that a rule removed it.

## Scaling, if libraries get large

These only earn their complexity once a library is big enough to feel the cost. None of them changes behavior today.

- Per-row out-of-band swaps instead of re-rendering a whole section on a change (previously ADR-0024, now covered by ADR-0034). A change deep in a large author section currently replaces the entire section's DOM. Per-row swaps need stable per-folder ids and add/remove plumbing.
- Render memoization (ADR-0022). The view renders on every read, not just on a cache miss, so a session that swaps sections repeatedly within one TTL window multiplies the render cost. Memoize only if that multiplier ever bites.
- Pruning the directory index (ADR-0020). The index never removes the entry for a folder deleted or renamed on disk, so memory grows with folder churn over the process's uptime, not with library size. A lingering entry is never read and cannot corrupt a scan, and every restart resets the index to empty, so this only matters on a high-churn library left running for a very long time. The fix is a root-scoped prune at the end of each walk.
