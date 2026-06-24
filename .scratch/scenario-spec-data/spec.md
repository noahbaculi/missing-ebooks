# Scenarios: declarative `ScenarioSpec` data

Architecture-review candidate #5. Replace the imperative `build_*` helpers in `src/scenarios.rs` with a `ScenarioSpec` data tree and one `materialize(spec, base)` walker. Each scenario becomes a `fn() -> ScenarioSpec` whose return value describes the library's shape; `materialize` does the `std::fs` work.

## Background

`src/scenarios.rs` is 669 lines. Five of the six builders (`build_mixed_forest`, `build_messy_shelf`, `build_clean_error`, `build_root_flagged`, `build_pre_marked`) are straight-line `touch(&root.join("a/b/c.mp3"))` chains. The sixth (`build_big_library`) is a generator: 50 authors with names assembled from name pools, 1..=12 books each, modulo-based coverage cadence, plus a handful of fixed-name anchors. Annotations like "the `(Unabridged)` suffix is stripped from the search query" sit beside the `touch` calls and explain why an entry exists.

The current `Scenario` struct exposes `build: fn(&Path) -> Vec<PathBuf>`. Consumers (`examples/explore.rs:161`, `src/bin/demo.rs:55`, `tests/sse.rs` x3, `tests/cache_render_byte_equal.rs:20`, `src/autosync.rs:496`, `src/service.rs:371`) call `(scenario.build)(base)` to seed and get the library roots back.

`tests/curated_contract.rs` does not use `scenarios`. It reads `tests/fixtures/curated/Audiobooks/` and `tests/fixtures/curated/expected.json` directly and stays unchanged here.

The `pub(crate) touch` helper has many call sites unrelated to scenarios (in `state.rs`, `scanner.rs`, `web.rs`, `service.rs`, `web/render.rs`, `demo/handlers.rs`). It stays as-is.

## Goal

Make the shape of each scenario diffable as data, and expose `Scenario.spec` so any future consumer can introspect what a scenario claims to produce without seeding a temp directory.

## Data model

In `src/scenarios.rs`:

```rust
pub enum MarkerKind {
    NoEbook,         // file name: ".no_ebook"
    EbookElsewhere,  // file name: ".ebook_elsewhere"
}

pub enum Entry {
    Folder { name: String, items: Vec<Entry> },
    Audio { name: String },   // full filename, e.g. "01 - Dune.mp3"
    Ebook { name: String },   // full filename, e.g. "Dune.epub"
    Marker(MarkerKind),
}

pub struct RootSpec {
    pub name: String,
    pub items: Vec<Entry>,
}

pub enum RootPlan {
    Created(RootSpec),          // materialize seeds it
    Uncreated { name: String }, // reserve the path, skip materialize
}

pub struct ScenarioSpec {
    pub roots: Vec<RootPlan>,
}
```

`Audio` and `Ebook` carry the full filename including extension. The variant communicates intent so a reader sees the audio leaf without parsing the suffix; scanner behavior still keys off the extension in `name`.

`Marker` stores only its kind. The leading dot lives in `materialize`, so "forgot the dot" is unrepresentable.

`RootPlan::Uncreated` covers `clean-error`'s second root, a path handed to `library_roots` but never created so canonicalization fails and the section renders as Error.

`String` and `Vec` everywhere: `build_big_library` constructs names at runtime, so the tree cannot be `&'static`.

## Materializer

```rust
pub fn materialize(spec: &ScenarioSpec, base: &Path) -> Vec<PathBuf> {
    spec.roots
        .iter()
        .map(|plan| match plan {
            RootPlan::Created(root) => {
                let path = base.join(&root.name);
                for item in &root.items {
                    write_entry(&path, item);
                }
                path
            }
            RootPlan::Uncreated { name } => base.join(name),
        })
        .collect()
}

fn write_entry(parent: &Path, entry: &Entry) {
    match entry {
        Entry::Folder { name, items } => {
            let dir = parent.join(name);
            mkdirs(&dir);
            for item in items {
                write_entry(&dir, item);
            }
        }
        Entry::Audio { name } | Entry::Ebook { name } => {
            touch(&parent.join(name));
        }
        Entry::Marker(kind) => {
            let file = match kind {
                MarkerKind::NoEbook => ".no_ebook",
                MarkerKind::EbookElsewhere => ".ebook_elsewhere",
            };
            touch(&parent.join(file));
        }
    }
}
```

`touch` and `mkdirs` keep their current signatures and call sites. `write_entry` is private; the public surface is `materialize`.

## `Scenario` change

```rust
pub struct Scenario {
    pub name: &'static str,
    pub description: &'static str,
    pub spec: fn() -> ScenarioSpec,  // was: build: fn(&Path) -> Vec<PathBuf>
}
```

Each consumer becomes one line:

```rust
// before
let roots = (scenario.build)(base);
// after
let roots = materialize(&(scenario.spec)(), base);
```

Eight call sites change: `examples/explore.rs:161`, `src/bin/demo.rs:55`, `src/autosync.rs:496`, `src/service.rs:371`, `tests/sse.rs:44`, `tests/sse.rs:106`, `tests/sse.rs:161`, `tests/cache_render_byte_equal.rs:20`.

## Builders

Small constructors keep call sites terse:

```rust
fn folder(name: &str, items: Vec<Entry>) -> Entry {
    Entry::Folder { name: name.into(), items }
}
fn audio(name: &str) -> Entry { Entry::Audio { name: name.into() } }
fn ebook(name: &str) -> Entry { Entry::Ebook { name: name.into() } }
fn no_ebook() -> Entry { Entry::Marker(MarkerKind::NoEbook) }
fn elsewhere() -> Entry { Entry::Marker(MarkerKind::EbookElsewhere) }
fn root(name: &str, items: Vec<Entry>) -> RootPlan {
    RootPlan::Created(RootSpec { name: name.into(), items })
}
fn uncreated(name: &str) -> RootPlan { RootPlan::Uncreated { name: name.into() } }
```

`build_clean_error` then reads:

```rust
fn build_clean_error() -> ScenarioSpec {
    ScenarioSpec {
        roots: vec![
            root("Covered Library", vec![
                folder("Author", vec![
                    folder("Book", vec![
                        audio("01 - Book.mp3"),
                        ebook("Book.epub"),
                    ]),
                ]),
            ]),
            uncreated("Missing Library"),
        ],
    }
}
```

`build_big_library` keeps its generator. The loop builds `Vec<Entry>` for the bulk and another for the fixed-name anchors, wraps them in a single `RootPlan::Created`, and returns the `ScenarioSpec`. No `std::fs` work runs until `materialize`.

`build_root_flagged` puts an `audio(...)` directly under `RootSpec.items`, with no enclosing `Folder`. ADR-0005 is exercised by that shape: loose root audio surfaces the root itself.

The existing per-entry annotations stay attached to the `folder`/`audio`/`ebook` calls where they apply. Code-comments style: explain why this entry exists, not what `folder` does.

## Tests

The existing builder tests (`mixed_forest_flags_the_expected_leaves`, `messy_shelf_flags_the_expected_leaves`, `clean_error_has_a_covered_root_and_an_uncreated_root`, `root_flagged_surfaces_the_root_itself`, `pre_marked_drops_covered_folders_and_keeps_click_targets`, `big_library_has_the_expected_flagged_count_and_anchor_states`, `big_library_generation_is_deterministic`) all stay. Each one becomes:

```rust
let spec = build_x();
let roots = materialize(&spec, dir.path());
// existing assertions on `flagged(&roots[0])` unchanged
```

`catalog_lists_all_six_scenarios` and `find_scenario_matches_by_name_and_rejects_unknown` keep their current bodies.

Add one new test for the materializer's leaf encoding:

```rust
#[test]
fn materialize_writes_marker_files_with_leading_dot() {
    let dir = tempfile::tempdir().unwrap();
    let spec = ScenarioSpec {
        roots: vec![root("R", vec![
            folder("F", vec![audio("a.mp3"), no_ebook(), elsewhere()]),
        ])],
    };
    materialize(&spec, dir.path());
    assert!(dir.path().join("R/F/.no_ebook").is_file());
    assert!(dir.path().join("R/F/.ebook_elsewhere").is_file());
    assert!(dir.path().join("R/F/a.mp3").is_file());
}
```

And one for `RootPlan::Uncreated`:

```rust
#[test]
fn materialize_returns_uncreated_paths_without_touching_disk() {
    let dir = tempfile::tempdir().unwrap();
    let spec = ScenarioSpec { roots: vec![uncreated("Missing")] };
    let roots = materialize(&spec, dir.path());
    assert_eq!(roots, vec![dir.path().join("Missing")]);
    assert!(!roots[0].exists());
}
```

## Non-goals

- No `expected_flagged(spec)` derivation. The round-trip property test was considered and dropped: it would encode marker and cover semantics in the spec evaluator alongside the scanner, duplicating logic.
- No change to `tests/curated_contract.rs` or its fixture. That test uses on-disk JSON, not `scenarios`.
- No change to `touch` or `mkdirs` visibility or signature.
- No change to ADRs.
- No `&'static` data model. `build_big_library`'s runtime name construction rules it out, and a mixed static-or-owned model would add a borrow lifetime to `Entry` for no payoff.

## Deletion test

Removes roughly 480 lines of `touch(&root.join("..."))` from the five hand-rolled builders and replaces them with about 350 lines of `folder`/`audio`/`ebook`/`no_ebook`/`elsewhere` calls plus the new `materialize` and constructors (about 90 lines). Net source is smaller. The structural win is what matters: the shape of `messy-shelf` and `mixed-forest` is visible at a glance, and `Scenario.spec` is introspectable for any later consumer.

No existing test simplifies. The README's "31 KB" headline number included generated bulk that stays. The honest payoff is diffability and an open door, not deletion.

## ADR notes

None. ADR-0005 (loose root audio surfaces the root itself) is exercised the same way as today, by putting audio directly under `RootSpec.items` with no enclosing `Folder`. No new ADR needed.

## Composition with the rest of the architecture review

Independent of #1-#4 and #7 (all done) and #6 (open, speculative). #5 lands on its own.
