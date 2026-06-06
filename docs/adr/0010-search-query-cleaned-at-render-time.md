# v1 search queries are cleaned from the folder name at render time

A search link's `{query}` is the folder name, cleaned by a pure function and
percent-encoded when the link template is filled. The cleaning runs at render
time in its own module (`query::clean_query`), not at scan time, and the view
stores no query.

This fits how the rest of the read path works. The cache holds only the
`FlaggedView` data, and every request re-renders from it in microseconds (see the
web-server design). Cleaning a folder name is a cheap pure string transform, so
running it per render costs nothing measurable and keeps `FlaggedView` free of a
presentation concern. Storing a query on each node would bloat the cached view
and tie the view model to one rendering detail.

The deferred tag-based-query increment is the one that moves query building to
scan time and stores the result on the view, because reading embedded audio tags
is filesystem work that cannot run cheaply per render on a networked mount (see
"Deferred: tag-based search queries" in the web-server design). That increment
gets its own ADR; v1 does not pre-pay for it.

The cleaning algorithm itself (drop bracketed segments, normalize `_` and `.` to
spaces, collapse whitespace, trim a dangling `-`, fall back to the raw name) is
specified in the search-links design and the web-server design's "Search-link
queries" section, so it is not repeated here.
