# v1 runtime writes: surgical marker updates, no runtime config mutation

v1 has exactly one runtime write: the marker file. Writing a marker updates the
cached view in place (remove the marked folder's subtree, prune now-empty
parents) instead of invalidating the cache and rewalking. This is provably
equivalent to a rescan, because a marker covers the folder and everything beneath
it, and it keeps a click instant on a large networked library where a full
rewalk costs seconds. The TTL still triggers a real rescan to pick up changes
made outside the app.

Concurrency. The equivalence above holds for a single writer; the cache has two
(a marker write and a TTL rescan) running on different tasks, so the two must not
interleave or one clobbers the other. All cache mutations are serialized under one
lock: a marker write that arrives during an in-flight rescan waits for the scan to
store its fresh view, then applies its subtree removal to that view. Because the
marker file is already on disk, every later rescan agrees too, so the equivalence
holds under concurrency. Reads stay cheap (clone an `Arc<FlaggedView>` under a
brief lock, or `arc-swap` for lock-free reads with the mutation lock on the write
side only); the single-flight is that same mutual exclusion plus a scan-in-progress
guard so concurrent stale reads await one scan rather than launching several. A
marker write does not refresh the cache timestamp: it changes the data, not the
freshness clock, so the TTL backstop still fires on schedule to pick up external
changes. The cost is that a marker click can block for the duration of an in-flight
rescan; acceptable because marks and rescans are both rare and this is single-user.

Config is immutable at runtime. Both `excluded_dirs` and `exclude_globs` are read
at startup and changed only by hand-editing `config.toml` and restarting, like
the reference script's hardcoded sets. The UI exclude button from the original
design is deferred: it would have required a `toml_edit` rewrite, a
`RwLock<Config>`, a `POST /exclude` route, and invalidate-and-rewalk per click.
Deferring lets v1 hold config behind a plain `Arc<Config>` with no lock and drops
the `toml_edit` dependency. The service layer keeps adding the button later a
thin change. The trade-off is convenience: excluding a series means editing TOML
and restarting rather than clicking.

Supersedes the original design's "any write invalidates the cache immediately."

Update (2026-06-04): ADR-0001 listed editability as a second difference between
the two exclude mechanisms (exclude names UI-editable, exclude globs hand-edited
only). The env-first config in ADR-0004 makes a UI write-back to the file
awkward, so editability is no longer treated as a distinguishing axis: the spec
and CONTEXT.md now describe the two as differing only in match criterion. The
names-only UI exclude button stays deferred as described above; it is simply not
documented as an editability difference.
