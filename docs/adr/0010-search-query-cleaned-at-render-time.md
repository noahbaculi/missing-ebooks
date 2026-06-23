# Search-link queries are cleaned from the folder name at render time

A search link's `{query}` placeholder is filled with the folder name, cleaned by a pure function (`query::clean_query`) and percent-encoded as the link template is built. The cleaning runs at render time and the cached scan view stores no query string.

This fits the rest of the read path. The cache holds the raw scan output (ADR-0022) and every request renders from it in microseconds. Cleaning a folder name is a cheap string transform, so running it per render costs nothing measurable and keeps the cached data free of a presentation concern. Storing a query on each node would bloat the cache and tie the cached view model to one rendering detail.

If a future search experience needs the query to come from somewhere other than the folder name (embedded audio tags are the obvious candidate), that work belongs at scan time rather than at render time, because reading tags is filesystem work that cannot run cheaply per render on a networked mount. Building it would store the query on the cached node and skip `clean_query` for the populated case. That change is not load-bearing today and has not been written.

The cleaning algorithm (drop bracketed segments, normalize `_` and `.` to spaces, collapse whitespace, trim a dangling separator, fall back to the raw name on an emptied result) lives in `src/query.rs` with its tests; treat that file as the spec.
