# Release the cache mutex around `rebuild_section` I/O

Status: ready-for-human

## Context

`Cache::rebuild_root` (`src/state.rs:145-169`) holds the entries mutex
across `rebuild_section().await`, which performs synchronous filesystem
I/O on a blocking task. While one root is being rescanned for undo,
reads against other roots stall on the same lock.

## Why this is deferred

The lock-across-I/O behavior matches the documented cold-slot single-
flight property (ADR-0022): two requests racing a cold cache must not
double-walk. Naively dropping the lock around the rescan would re-open
that race. Solving both requires per-root locking or an in-flight token
map, both of which add complexity beyond the rework's scope.

## Possible directions

1. Per-root locks: a `Vec<Mutex<RootSlot>>` sized to `config.library_roots.len()`,
   with the slot mutex acquired per operation. Loses cross-root atomicity
   for marker writes that touch multiple roots (none today, but flag if
   one is ever added).
2. In-flight token map: a `HashMap<usize, Arc<Notify>>` keyed by root index
   that lets a second caller wait on the first's rescan without holding
   the entries mutex.
3. Optimistic compare-and-swap on the per-section slot, retrying on
   conflict.

## Scope when picked up

Pick a direction, write the spec, brainstorm gate criteria (a benchmark
that demonstrates the cross-root stall today, then proves it gone after
the change).

## Out of scope

Anything that changes ADR-0022's cold-slot single-flight contract.

## Comments
