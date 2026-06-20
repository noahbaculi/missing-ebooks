# Iterator-based `reduce_to_flagged` to eliminate one render-path clone

Status: ready-for-human

## Context

ADR-0022 moved tree rendering onto the per-request read path. The gaps
render walks two clone steps:

1. `scanner::reduce_to_flagged` (`src/scanner.rs:224`) clones every
   matching `ScannedFolder` into a fresh `Vec`, including its
   `cover_files` and `audio_files` `Vec<String>` fields.
2. `tree::build`'s `insert_all` (`src/tree.rs:141-142`) clones those
   same `cover_files` and `audio_files` Vecs into the `Node` it pushes.

The first clone is avoidable. Returning an iterator over borrowed
`&ScannedFolder` from `reduce_to_flagged` and updating `tree::build` to
accept `impl IntoIterator<Item = &ScannedFolder>` skips the intermediate
Vec.

## Why this is deferred

Measured perf in ADR-0022 sits well under the documented gates:
0.086 ms on the mixed-forest reference scenario and 3.758 ms on the
worst synthetic 10k-folder row, against gates of 2 ms and 25 ms
respectively. The cost is real but the budget is not threatened.

## Scope

- Change `reduce_to_flagged`'s signature to return `impl Iterator<Item =
  &ScannedFolder>`.
- Change `tree::build`'s signature to accept `impl IntoIterator<Item =
  &ScannedFolder>`.
- Update call sites: `src/service.rs::render_section`, the
  `examples/tree_bench.rs` benchmark, and any tests.
- Re-run scan_bench and tree_bench; record the new numbers in a follow-
  up ADR or update ADR-0022's evidence paragraph if the deltas are large
  enough to matter.

## Out of scope

Changing `Node`'s field types to avoid the second clone. That is a much
larger change with wide blast radius; revisit only if a real workload
threatens a render gate.

## Comments
