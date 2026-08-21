# The directory index grows until restart rather than pruning vanished folders

Date: 2026-06-15.

## Context

The scanner keeps a per-directory index (`scanner::DirIndex`, a `HashMap` from a directory's path to its cached mtime, subdirs, and filenames), held in `AppState` as `Arc<Mutex<DirIndex>>`. Every scan path reads and writes it. Only a `/rescan` click discards it before the walk.

## Decision

It only ever inserts an entry when a directory is listed and removes one when this process writes or deletes a marker (`invalidate_index`). It never bulk-prunes. So when a folder is deleted or renamed on disk, its parent re-lists on the next walk because its mtime moved, drops the folder from the cached `subdirs`, and the walk never visits it again, but the folder's own entry lingers in the map. The index is in-memory only and rebuilt empty on restart, so that restart is its effective bound.

## Consequences

A lingering entry is never read. A lookup happens only for a path the walk reaches, and the walk reaches a child only through its parent's cached `subdirs`, so an orphaned entry cannot change a scan's output. It costs memory: a few hundred bytes plus the path and filename strings per dead folder, accruing with cumulative folder churn over the process's uptime rather than with the library's size. On a self-hosted library, where folders are added occasionally and deleted rarely, that is kilobytes over weeks, reclaimed on the next restart.

We considered pruning at the end of each walk by dropping any key the walk did not visit. One `DirIndex` is shared across every library root and both view modes, but a single walk covers one root and knows only that root's visited set. Dropping every unvisited key would evict every other root's entries on each scan, forcing a cold re-walk of those roots next time and thrashing the cache the feature exists to fill. The safe version is a root-scoped retain that keeps keys outside the walked root plus the ones visited under it, which is more machinery than a kilobyte-scale, low-churn, restart-bounded leak earns now. We accept the growth for v1 and reach for the root-scoped prune if uptime numbers on a high-churn library ever show it matters.
