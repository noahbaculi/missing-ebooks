# Architecture review, June 2026

Seven candidate deepening opportunities surfaced by `improve-codebase-architecture` on 2026-06-23. Each names the seam, the friction, an "after" sketch, the deletion-test result, and any ADR collision. Candidate #1 is implemented; the rest remain as starting points for future brainstorming.

Vocabulary follows `CONTEXT.md` (library root, flagged folder, container, covered, marker, render cache, dir index, warm/cold scan). Architectural terms (module, seam, depth, locality, deletion test) follow the codebase-design conventions.

## Status

| # | Candidate | Strength | Status |
|---|---|---|---|
| 1 | `scanner::scan_root` owning canonicalize + classify + `RootScan` | Strong | **done** (see `.scratch/scanner-scan-root/`) |
| 2 | `RawViewStore` replacing the four cache primitives | Strong | **done** (see `.scratch/raw-view-store/`, ADR-0027) |
| 3 | `Autosync::attach` collapsing snapshot/seed/subscribe | Strong | open |
| 4 | `tree.rs` owns the ADR-0005 `.`-node rule end-to-end | Worth exploring | open |
| 5 | Declarative `ScenarioSpec` data | Worth exploring | open |
| 6 | `Renderer` context object | Speculative | open |
| 7 | Single `RawView::sections(mode)` iterator | Worth exploring | open |

The Strong trio (#1, #2, #3) composes: #1's `RootScan` substrate makes #2's API simpler, which gives #3 a clean substrate to render from. #7 falls out naturally during #3. #4-#6 are independent.

The original HTML report with before/after diagrams lives at `/tmp/architecture-review-20260623-154854.html` until the next reboot. Regenerate by re-running the architecture review skill.

---

## 2 · `RawViewStore` replaces the four-method cache surface

**Strength:** Strong.

**Files:**
- `src/state.rs::Cache` (L64-190)
- `src/service.rs::mark` (L159-203)
- `src/service.rs::unmark` (L208-259)
- `src/state.rs::apply_marker_or_build`, `rebuild_root`

**Problem.** The cache exposes four closure-taking primitives (`get_or_build`, `rebuild`, `apply_marker_or_build`, `rebuild_root`). `service::mark` and `service::unmark` are 40-line ceremonies that clone `Arc<Config>`, `Arc<ScanSettings>`, `Arc<Mutex<DirIndex>>` two or three times to feed those closures. The cache method `apply_marker_or_build` already *names* a marker but does not write one. The seam is deeper than the name says but shallower than the caller wants. Cache tests are dominated by `Arc::ptr_eq` assertions probing slot identity (`state.rs:744-770`).

**After.** A `RawViewStore` owns `Cache + Arc<Mutex<DirIndex>> + Arc<Config> + Arc<ScanSettings>`. Public surface:

- `current() -> Arc<RawView>`
- `rescan() -> Arc<RawView>` (clears the dir index, per ADR-0020)
- `apply_mark(root, rel, marker) -> Arc<RawView>` (writes the marker file, invalidates the dir entry, edits the slot in place per ADR-0002)
- `undo_mark(root, rel, marker) -> Arc<RawView>` (mirror, using per-root rebuild)

Tests assert observable behaviour ("two marks against a warm store do not re-scan the disk") rather than slot pointer identity.

**Deletion test:** passes. The closure-cloning at `service.rs:234-256` disappears. Single-flight, TTL, and in-place-edit semantics concentrate behind one type. `apply_marker_or_build`'s name no longer leaks domain into the cache.

**ADR notes.** ADR-0002 (marker writes edit cache in place), ADR-0020 (dir index grows until restart), ADR-0022 (cache holds raw scan output) all constrain behaviour, not API shape. None blocks.

**Composes with #1.** With `RootScan` already in place at the cache boundary, the per-root field on `RawViewStore` reads cleanly without the bridge that #1 had to introduce.

---

## 3 · `Autosync::attach` collapses the subscribe/snapshot/seed dance

**Strength:** Strong.

**Files:**
- `src/web.rs::events` (L210-240)
- `src/autosync.rs::snapshot_and_seed` (L89-102)
- `src/autosync.rs::subscribe_and_seed` (L178-186)
- `src/autosync.rs::compute_pushes` (L28-57)
- `src/demo/handlers.rs::events` (L267)

**Problem.** Each SSE handler performs an ordered four-step dance: fetch raw, snapshot and hash, send the `snapshot` event, register with the seeded hashes. If steps 3 and 4 swap, a tab can receive a `section` event before its snapshot and double-paint. The ordering invariant lives only in handler discipline, in two places, and the demo variant has already drifted (no autosync loop). `compute_pushes` is generic over a renderer purely so a test can stub it; in production exactly one closure ever flows in.

**After.** One call: `autosync::attach(state: &Arc<AppState>, mode: ViewMode, sender: SseSender) -> impl Stream`. Inside, a private `SectionDiffer { last_hash, render_fn }` owns the per-mode hash baselines and the render closure once. `tick(&raw)` yields pushes, `snapshot(&raw)` yields the initial OOB payload, and both share a body. The "snapshot before subscribe" invariant lives next to the registry.

**Deletion test:** passes. Removes ~30 lines of caller-side orchestration from both `web::events` and `demo::handlers::events`, deletes the `R: FnMut(...)` generic from `compute_pushes`, and concentrates the ordering invariant where the registry lives.

**ADR notes.** ADR-0023 (autosync only runs while subscribed) and ADR-0024 (autosync section-level OOB swap) constrain what the loop does, not who calls what. Compatible.

**Composes with #1 and #7.** With `RawView = Vec<RootScan>` already in place, the differ iterates `RootScan`s directly. If #7 also lands, the snapshot and tick paths share one iterator.

---

## 4 · `tree.rs` owns the ADR-0005 `.`-node rule end-to-end

**Strength:** Worth exploring.

**Files:**
- `src/tree.rs::build` (L67-108, the `root_entry` branch at L87-106)
- `src/service.rs::render_root_state` (L429-456, where the `root_name = Path::new(path).file_name()` extraction lives, now `canonical_path.file_name()` after #1)

**Problem.** ADR-0005 says a library root can itself be a flagged folder when loose audio sits in it. The rule's two halves live apart. `tree.rs:87-106` knows that an empty `rel_path` `ScannedFolder` turns into a pinned node with `rel_path = "."` and a name supplied by the caller. `service.rs:438-441` derives that name by `Path::new(path).file_name()`. Renderer tests for the `.` node and scanner tests for the loose root assert via different paths. Three callers pass the name (production from canonical path, demo handler, unit tests with `"Audiobooks"`), and the name is not validated to actually match the root.

**After.** `tree.rs::build` accepts the canonical root path (`&Path`) and derives the display name itself. It returns `RootForest { root_audio: Option<Node>, children: Vec<Node> }` instead of `Vec<Node>`. The "pinned first" invariant is a type-level field rather than an `insert(0, ...)` call. The renderer iterates `forest.iter()` without knowing the `.`-rel_path convention.

**Deletion test:** passes. Concentrates the ADR-0005 rule into one module. `tree.rs` gains ~5 lines, `service.rs` loses the path-parsing dance and the `root_name` propagation.

**ADR notes.** ADR-0005 is the rule, not the shape. Compatible.

---

## 5 · Declarative `ScenarioSpec` data instead of imperative `build_*`

**Strength:** Worth exploring.

**Files:** `src/scenarios.rs` L81-669:
- `build_mixed_forest` (L81, 171 lines of `touch(...)` calls)
- `build_messy_shelf` (L252)
- `build_clean_error` (L322)
- `build_root_flagged` (L335)
- `build_pre_marked` (L345)
- `build_big_library` (L362)

**Problem.** 31 KB of hand-rolled `touch(...)` chains form a tiny DSL hidden in straight-line Rust. The shape of "mixed-forest" is invisible without reading 170 lines, "messy-shelf" cannot be diffed against it without skimming both, and expected-flagged-set assertions live elsewhere (scattered through `scanner.rs::tests` and the demo end-to-end tests). Neither `examples/explore.rs` nor `tests/curated_contract.rs` can introspect what a scenario *claims* to produce without running a scan.

**After.** Each scenario is a `&'static ScenarioSpec` carrying root layouts as nested data:

```rust
RootSpec {
    name: "Audiobooks",
    items: &[
        Folder { name: "Karen Cleveland", items: &[/* ... */] },
        AudioFile { name: "01.mp3" },
        /* ... */
    ],
}
```

One `materialize(spec, base)` walks it. A second `expected_flagged(spec)` derives the expected gap set from the same data, so the curated contract test becomes "for every scenario, `scan(materialize(s)) == expected_flagged(s)`". The interface becomes the test surface, the deeper-module payoff. `build_big_library` becomes a generator expression instead of a loop.

**Deletion test:** passes. Removes most of the 31 KB and gives the explore harness, the curated contract, and demo state seeding one source of truth.

**ADR notes:** none.

---

## 6 · `Renderer` context replaces `(links, mode, counter)` plumbing

**Strength:** Speculative.

**Files:** `src/web/render.rs` —
- `render_section` (L258)
- `render_node` (L391)
- `marker_buttons` (L505)
- `row_actions` (L479)
- `search_links` (L559)
- `single_oob_section` (L37)
- `oob_sections` (L54)
- `roots` (L24)

**Problem.** Nine helpers carry the same two scalars (`links: &[SearchLink]`, `mode: ViewMode`) and most also carry `counter: &Cell<usize>` for DOM-id uniqueness. The signatures are noisy. The counter's reset contract (once per `render_section`) is enforced only by a comment at L553. `mode == ViewMode::All` branches scattered across `status_icon`, `cover_files_span`, `smell_label`, `marker_buttons`, `render_node` repeat the conditional rather than dispatching.

**After.** A `Renderer<'a> { mode: ViewMode, links: &'a [SearchLink], counter: Cell<usize> }` constructed once per `render_section`. Helpers become methods. `next_id(prefix, root)` hides the counter, the per-render contract becomes type-enforced. Mode-conditional helpers could even split into `gaps` and `show_all` impls if a future strategy split is wanted.

**Deletion test:** borderline. This mostly *relocates* the `(links, mode, counter)` plumbing onto `self`, the classic shallow-relocation failure mode. Payoff is locality of the counter contract, not concentration of complexity. Worth doing only as part of a larger render redesign (paired with #7, say).

**ADR notes:** none.

---

## 7 · Single `RawView::sections(mode)` iterator shared by service, autosync, snapshot

**Strength:** Worth exploring.

**Files:**
- `src/service.rs::render_view` (L403-407)
- `src/service.rs::render_section_from_raw` (L415-424)
- `src/autosync.rs::render_oob_section` (L74-82)
- `src/autosync.rs::snapshot_and_seed` (L89-102)
- `src/web/render.rs::oob_sections` (L54-60)

**Problem.** Producing one rendered section appears in three places, each with its own iteration: per-request render, autosync snapshot, autosync diff loop. `audiobook_count` runs per section per iteration. ADR-0024 demands byte equality between SSE and rescan paths; today that is a property enforced by careful authorship rather than by sharing the loop.

**After.** A `RawView::sections(mode) -> impl Iterator<Item = RenderedSection>` (or free function) yields each rendered section once. Snapshot collects OOB-wrapped strings, the diff stage hashes, the per-request renderer collects into `FlaggedView`. The byte-equality invariant becomes a structural guarantee.

**Deletion test:** passes. Three loops collapse to one; the renderer generic on `compute_pushes` goes away.

**ADR notes.** ADR-0024 constrains the body, not the shape. Compatible.

**Composes with #3.** Together they make `Autosync::attach` trivial.

---

## Suggested next pick

Open Strong candidates: #2 and #3. They compose with the now-shipped #1. If both feel large, #4 is the smallest open candidate and lands a real piece of the ADR-0005 rule into one module.
