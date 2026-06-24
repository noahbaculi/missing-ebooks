# Scenarios: declarative `ScenarioSpec` data — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the imperative `build_*` helpers in `src/scenarios.rs` with a `ScenarioSpec` data tree plus one `materialize(spec, base)` walker, and expose `Scenario.spec` to consumers.

**Architecture:** Add a typed-leaf data model (`Entry`, `MarkerKind`, `RootSpec`, `RootPlan`, `ScenarioSpec`) and a recursive `materialize` walker. Rewrite the six `build_*` functions to return `ScenarioSpec` instead of seeding the disk directly. Flip `Scenario.build: fn(&Path) -> Vec<PathBuf>` to `Scenario.spec: fn() -> ScenarioSpec` and update every consumer in one atomic task.

**Tech Stack:** Rust, `tempfile`, `std::fs`.

## Global constraints

- Code-comments style (see `writing-style-code-comments`): terse, verb-first or noun-phrase doc summaries, no em dashes, no "This function" openers, backtick identifiers.
- `touch` and `mkdirs` keep their current signatures and visibility. Their non-scenario callers are out of scope.
- `tests/curated_contract.rs` and `tests/fixtures/curated/` are out of scope.
- Commits follow Conventional Commits. Granular, no squashing.
- Pre-commit hook runs fmt, clippy, `cargo doc -D warnings`. Never bypass with `--no-verify`.

---

## Task 1: Add the data model, materializer, and constructors

Introduces the new types and the walker, plus their direct tests. Old `build_*` functions and `Scenario.build` stay in place: this task adds, does not migrate.

**Files:**
- Modify: `src/scenarios.rs` (add types, `materialize`, `write_entry`, constructors, two new tests)

**Interfaces produced:**
- `pub enum MarkerKind { NoEbook, EbookElsewhere }`
- `pub enum Entry { Folder { name: String, items: Vec<Entry> }, Audio { name: String }, Ebook { name: String }, Marker(MarkerKind) }`
- `pub struct RootSpec { pub name: String, pub items: Vec<Entry> }`
- `pub enum RootPlan { Created(RootSpec), Uncreated { name: String } }`
- `pub struct ScenarioSpec { pub roots: Vec<RootPlan> }`
- `pub fn materialize(spec: &ScenarioSpec, base: &Path) -> Vec<PathBuf>`
- Constructors `folder`, `audio`, `ebook`, `no_ebook`, `elsewhere`, `root`, `uncreated` (module-private, used by Task 2 and by the new tests)

- [ ] **Step 1: Write the failing leaf-encoding test**

  Append to the `mod tests` block in `src/scenarios.rs`:

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

- [ ] **Step 2: Write the failing uncreated-root test**

  Append directly below Step 1's test:

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

- [ ] **Step 3: Run the tests and verify they fail**

  Run: `cargo test --lib scenarios::tests::materialize_`
  Expected: compile errors on `ScenarioSpec`, `root`, `folder`, `audio`, `no_ebook`, `elsewhere`, `uncreated`, `materialize`.

- [ ] **Step 4: Add the data types**

  Insert just below the existing `use` block and above `pub struct Scenario`:

  ```rust
  /// Marker file kind a scenario can drop into a folder. The dot prefix lives
  /// in `materialize`, so a missing dot is unrepresentable.
  pub enum MarkerKind {
      NoEbook,
      EbookElsewhere,
  }

  /// One node in a `ScenarioSpec` tree.
  pub enum Entry {
      Folder { name: String, items: Vec<Entry> },
      /// Full audio filename including extension, e.g. `"01 - Dune.mp3"`.
      Audio { name: String },
      /// Full ebook filename including extension, e.g. `"Dune.epub"`.
      Ebook { name: String },
      Marker(MarkerKind),
  }

  /// A library root that `materialize` seeds under `base`.
  pub struct RootSpec {
      pub name: String,
      pub items: Vec<Entry>,
  }

  /// One root in a scenario. `Uncreated` reserves the path without touching
  /// disk, so canonicalization fails and the section renders Error.
  pub enum RootPlan {
      Created(RootSpec),
      Uncreated { name: String },
  }

  /// Declarative description of a synthetic library, walked by `materialize`.
  pub struct ScenarioSpec {
      pub roots: Vec<RootPlan>,
  }
  ```

- [ ] **Step 5: Add the materializer**

  Insert just below `touch`:

  ```rust
  /// Seed `spec` under `base` and return the library roots in spec order.
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

- [ ] **Step 6: Add the constructors**

  Insert just below `write_entry`. These are module-private. Task 2 uses them throughout the rewritten builders; the new tests use them too.

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

  The constructors are unused outside the new tests until Task 2. Add `#[cfg_attr(not(test), allow(dead_code))]` to each so clippy stays quiet:

  ```rust
  #[cfg_attr(not(test), allow(dead_code))]
  fn folder(name: &str, items: Vec<Entry>) -> Entry { /* ... */ }
  ```

  Apply the same attribute to `materialize` and to every constructor through `uncreated`. Task 2 removes them.

- [ ] **Step 7: Run the new tests and verify they pass**

  Run: `cargo test --lib scenarios::tests::materialize_`
  Expected: both `materialize_writes_marker_files_with_leading_dot` and `materialize_returns_uncreated_paths_without_touching_disk` pass.

- [ ] **Step 8: Run the full suite**

  Run: `cargo test`
  Expected: every existing test still passes. The old `build_*` and `Scenario.build` are untouched.

- [ ] **Step 9: Run clippy and doc**

  Run: `cargo clippy --all-targets -- -D warnings && cargo doc --no-deps -D warnings`
  Expected: clean.

- [ ] **Step 10: Commit**

  ```bash
  git add src/scenarios.rs
  git commit -m "feat(scenarios): add ScenarioSpec data model and materializer

  Introduces Entry, MarkerKind, RootSpec, RootPlan, ScenarioSpec, and the
  materialize walker that turns a spec into on-disk folders. Constructors
  (folder, audio, ebook, no_ebook, elsewhere, root, uncreated) are added
  module-private. Existing build_* helpers and Scenario.build are unchanged;
  Task 2 migrates them.

  No behavior change."
  ```

---

## Task 2: Migrate every builder and consumer to `Scenario.spec`

Flips `Scenario.build` to `Scenario.spec`, rewrites all six `build_*` helpers to return `ScenarioSpec`, and updates every consumer. Atomic because `Scenario` has one fn-pointer field.

**Files:**
- Modify: `src/scenarios.rs` (rename `Scenario.build` to `spec`, rewrite all six builders, update existing tests, drop the `cfg_attr` from Task 1's constructors)
- Modify: `examples/explore.rs` (one call site)
- Modify: `src/bin/demo.rs` (one call site)
- Modify: `src/autosync.rs` (one call site in tests)
- Modify: `src/service.rs` (one call site in tests)
- Modify: `tests/sse.rs` (three call sites)
- Modify: `tests/cache_render_byte_equal.rs` (one call site)

**Interfaces consumed (from Task 1):**
- Every type and function added in Task 1.

**Interfaces produced:**
- `pub struct Scenario { pub name: &'static str, pub description: &'static str, pub spec: fn() -> ScenarioSpec }`
- Each `build_x` now has signature `fn build_x() -> ScenarioSpec`.

- [ ] **Step 1: Rename `Scenario.build` to `spec` and change its type**

  In `src/scenarios.rs`, edit the `Scenario` struct:

  ```rust
  /// One catalog entry: a name, a one-line description, and the spec describing
  /// its library shape.
  #[derive(Clone, Copy)]
  pub struct Scenario {
      pub name: &'static str,
      pub description: &'static str,
      /// Returns the scenario's library spec. Run `materialize(&spec, base)` to
      /// seed it.
      pub spec: fn() -> ScenarioSpec,
  }
  ```

- [ ] **Step 2: Rewrite `build_clean_error`**

  Replace the function body with:

  ```rust
  /// Two roots side by side: one fully covered (Clean), one never created
  /// (Error).
  fn build_clean_error() -> ScenarioSpec {
      ScenarioSpec {
          roots: vec![
              // Every audio folder has an ebook beside it, so the root is Clean.
              root("Covered Library", vec![
                  folder("Author", vec![
                      folder("Book", vec![
                          audio("01 - Book.mp3"),
                          ebook("Book.epub"),
                      ]),
                  ]),
              ]),
              // Path handed to `library_roots` but never created. It cannot
              // canonicalize, so the section renders "Could not scan this root"
              // and the server logs the skip warning.
              uncreated("Missing Library"),
          ],
      }
  }
  ```

  Update the catalog entry's `build:` field to `spec:`.

- [ ] **Step 3: Rewrite `build_root_flagged`**

  ```rust
  /// Loose audio directly in the root, so the root itself is the gap (ADR-0005).
  fn build_root_flagged() -> ScenarioSpec {
      ScenarioSpec {
          roots: vec![
              // Audio loose in the root: the root itself is the gap, a single
              // flagged node with rel_path "." (see ADR-0005).
              root("Loose Audio", vec![
                  audio("01 - Some Lecture.mp3"),
                  audio("02 - Some Lecture.mp3"),
              ]),
          ],
      }
  }
  ```

  Update the catalog entry's `build:` field to `spec:`.

- [ ] **Step 4: Rewrite `build_pre_marked`**

  ```rust
  /// Pre-existing markers hide covered folders while sibling gaps stay actionable.
  fn build_pre_marked() -> ScenarioSpec {
      ScenarioSpec {
          roots: vec![
              root("Marked Library", vec![
                  folder("Marked Author", vec![
                      // Covered: carries `.no_ebook`, so it is absent.
                      folder("Covered Book", vec![
                          audio("01 - Covered Book.m4b"),
                          no_ebook(),
                      ]),
                      // No marker, so it stays a click target.
                      folder("Uncovered Book", vec![
                          audio("01 - Uncovered Book.m4b"),
                      ]),
                  ]),
                  // Series-level `.ebook_elsewhere` covers the whole subtree.
                  folder("Elsewhere Series", vec![
                      elsewhere(),
                      folder("Book A", vec![audio("01 - Book A.mp3")]),
                  ]),
                  // No markers, so Plain Book stays flagged.
                  folder("Plain Author", vec![
                      folder("Plain Book", vec![audio("01 - Plain Book.m4b")]),
                  ]),
              ]),
          ],
      }
  }
  ```

  Update the catalog entry.

- [ ] **Step 5: Rewrite `build_messy_shelf`**

  Mechanical translation of the existing `touch` chain. Preserve every original annotation, attached to the matching `folder`/`audio`/`ebook`/`no_ebook`/`elsewhere` call:

  ```rust
  /// A library a careless owner never tidied. Flagged folders land at depths 1,
  /// 2, and 3 in one tree, which `build_mixed_forest` never produces.
  fn build_messy_shelf() -> ScenarioSpec {
      ScenarioSpec {
          roots: vec![root("Audiobooks", vec![
              // Standalone books with no author folder above them, flagged at
              // the root's first level.
              folder("The Hobbit", vec![audio("01 - The Hobbit.mp3")]),
              folder("Neuromancer", vec![audio("01 - Neuromancer.m4b")]),
              // Andy Weir book left loose at the top instead of under "Andy Weir":
              // the same author filed two ways.
              folder("Project Hail Mary", vec![audio("01 - Project Hail Mary.mp3")]),
              // Dune carries its own epub, so it drops out of the tree.
              folder("Dune", vec![
                  audio("01 - Dune.mp3"),
                  ebook("Dune.epub"),
              ]),

              // Author folders that hold audio directly: the author folder itself
              // is the flagged leaf.
              folder("Stephen King", vec![audio("01 - The Gunslinger.mp3")]),
              folder("Neil Gaiman", vec![audio("01 - Coraline.m4a")]),

              // Half-sorted: one book loose in the author folder, another nested,
              // so the author folder and its book both flag.
              folder("Terry Pratchett", vec![
                  audio("01 - The Colour of Magic.mp3"),
                  folder("Going Postal", vec![audio("01 - Going Postal.m4b")]),
              ]),

              // Dumping containers whose names a tidy library would not use.
              folder("To Sort", vec![
                  folder("Some Download", vec![
                      audio("Becky Chambers - Record of a Spaceborn Few.mp3"),
                  ]),
                  folder("Another Rip", vec![
                      audio("Martha Wells - Network Effect.m4b"),
                  ]),
                  // A pile that grew a subfolder, so a gap sits three levels down.
                  folder("Box Set", vec![
                      folder("Disc 1", vec![audio("Title Sequence 01.mp3")]),
                  ]),
              ]),
              folder("Downloads", vec![
                  folder("Unknown Audiobook", vec![
                      audio("Ursula K. Le Guin - The Tombs of Atuan.mp3"),
                  ]),
              ]),

              // A normal author > book pair. Artemis stays flagged. The Martian
              // carries `.no_ebook`, so it drops out while its sibling stays.
              folder("Andy Weir", vec![
                  folder("Artemis", vec![audio("01 - Artemis.mp3")]),
                  folder("The Martian", vec![
                      audio("01 - The Martian.m4b"),
                      no_ebook(),
                  ]),
              ]),
              folder("Ursula K. Le Guin", vec![
                  folder("The Left Hand of Darkness", vec![
                      audio("01 - The Left Hand of Darkness.mp3"),
                  ]),
              ]),

              // A series container with no author above it: the owner filed the
              // series but not the writer.
              folder("The Expanse", vec![
                  folder("Leviathan Wakes", vec![audio("01 - Leviathan Wakes.mp3")]),
                  folder("Caliban's War", vec![audio("01 - Caliban's War.m4b")]),
                  folder("Abaddon's Gate", vec![audio("01 - Abaddon's Gate.mp3")]),
              ]),

              // Another series with no author above it, half-covered: The Great
              // Hunt is hidden by `.ebook_elsewhere`.
              folder("Wheel of Time", vec![
                  folder("The Eye of the World", vec![
                      audio("01 - The Eye of the World.mp3"),
                  ]),
                  folder("The Great Hunt", vec![
                      audio("01 - The Great Hunt.m4b"),
                      elsewhere(),
                  ]),
              ]),

              // The one meticulous pocket: a full author > series > book hierarchy
              // with two series, so flagged leaves reach depth 3.
              folder("Brandon Sanderson", vec![
                  folder("The Stormlight Archive", vec![
                      folder("The Way of Kings", vec![audio("01 - The Way of Kings.m4b")]),
                      folder("Words of Radiance", vec![audio("01 - Words of Radiance.mp3")]),
                  ]),
                  folder("Mistborn", vec![
                      folder("The Final Empire", vec![audio("01 - The Final Empire.m4b")]),
                  ]),
              ]),
          ])],
      }
  }
  ```

  Update the catalog entry.

- [ ] **Step 6: Rewrite `build_mixed_forest`**

  Same mechanical translation. Three roots: `Library`, `External Library`, `Complete Library`. Preserve every annotation from the existing builder. Read the current `build_mixed_forest` body (`src/scenarios.rs:81-246`) and translate every `touch(&root.join(...))` call into the equivalent nested `folder`/`audio`/`ebook`/`elsewhere`/`no_ebook` calls. Each Unicode character (`\u{2019}`, `\u{c9}`, `\u{ed}`) stays inline in the `name` string literal exactly as it appears today.

  The three roots become three `root(...)` entries in `roots: vec![...]`. Update the catalog entry.

- [ ] **Step 7: Rewrite `build_big_library`**

  Keep the generator. Build `Vec<Entry>` for the bulk and the anchors, then wrap in one `RootPlan::Created`. The name pools and modulo cadence stay byte-for-byte identical, so the generated set is deterministic and matches the existing `big_library_has_the_expected_flagged_count_and_anchor_states` test.

  Skeleton:

  ```rust
  /// About fifty authors of varying size and nesting, for layout testing at volume.
  fn build_big_library() -> ScenarioSpec {
      const FIRST_NAMES: [&str; 10] = [/* unchanged */];
      const LAST_NAMES: [&str; 7]   = [/* unchanged */];
      const TITLE_LEFT: [&str; 8]   = [/* unchanged */];
      const TITLE_RIGHT: [&str; 9]  = [/* unchanged */];
      const AUDIO_EXT: [&str; 3]    = ["mp3", "m4b", "m4a"];

      // Per-author folders accumulate here. Series-nested authors fold their
      // books under a series Folder before joining `authors`.
      let mut authors: Vec<Entry> = Vec::new();
      let mut g: usize = 0;

      for a in 0..50usize {
          let author_name = format!("{} {}", FIRST_NAMES[a % 10], LAST_NAMES[(a / 10) % 7]);
          let book_count = 1 + (a % 12);
          // Every sixth author nests its books under a series container.
          let series_name = a
              .is_multiple_of(6)
              .then(|| format!("{} Cycle", LAST_NAMES[(a / 10) % 7]));

          let mut books: Vec<Entry> = Vec::new();
          for b in 0..book_count {
              let title = format!(
                  "{} {}",
                  TITLE_LEFT[(a + b) % 8],
                  TITLE_RIGHT[(a * 2 + b) % 9],
              );
              let mut book_items: Vec<Entry> = vec![
                  audio(&format!("01 - {title}.{}", AUDIO_EXT[g % 3])),
              ];
              if g.is_multiple_of(5) {
                  // Covered: an ebook beside the audio, so the scanner drops it.
                  book_items.push(ebook(&format!("{title}.epub")));
              } else if g.is_multiple_of(7) {
                  // Pre-marked: alternate the two marker kinds for variety.
                  book_items.push(if g.is_multiple_of(2) { no_ebook() } else { elsewhere() });
              }
              books.push(folder(&title, book_items));
              g += 1;
          }

          let author_entry = match series_name {
              Some(series) => folder(&author_name, vec![folder(&series, books)]),
              None => folder(&author_name, books),
          };
          authors.push(author_entry);
      }

      // Fixed-name anchors pin specific coverage states for assertions.
      authors.push(folder("Flagged Anchor", vec![
          folder("A Plain Flagged Book", vec![audio("01 - track.mp3")]),
      ]));
      authors.push(folder("Covered Anchor", vec![
          folder("A Covered Book", vec![
              audio("01 - track.mp3"),
              ebook("A Covered Book.epub"),
          ]),
      ]));
      authors.push(folder("Marked Anchor", vec![
          folder("A Pre-Marked Book", vec![
              audio("01 - track.m4b"),
              no_ebook(),
          ]),
      ]));
      // One ancestor marker hides the whole subtree.
      authors.push(folder("Ancestor-Covered Collection", vec![
          elsewhere(),
          folder("Book One", vec![audio("01 - track.mp3")]),
          folder("Book Two", vec![audio("01 - track.mp3")]),
      ]));
      // A very long author and book name, to see how a wide row wraps.
      authors.push(folder(
          "A Very Long Author Name That Keeps Going For Layout Testing",
          vec![folder(
              "An Equally Long Book Title That Should Wrap Across The Line When The Window Is Narrow",
              vec![audio("01 - track.mp3")],
          )],
      ));
      // Deeply nested, non-ASCII: accents and a U+2019 right single quote that
      // survives query cleaning and is percent-encoded in the search href.
      authors.push(folder(
          "\u{c9}mile R\u{ed}os",
          vec![folder("The Collected Works", vec![folder("Inner Series", vec![folder(
              "Assassin\u{2019}s Apprentice (Unabridged)",
              vec![audio("01 - track.m4b")],
          )])])],
      ));

      ScenarioSpec {
          roots: vec![root("Audiobooks", authors)],
      }
  }
  ```

  Update the catalog entry.

- [ ] **Step 8: Drop the `cfg_attr(not(test), allow(dead_code))` from Task 1**

  Every constructor (`folder` through `uncreated`) and `materialize` now have non-test callers, so the attribute is no longer needed. Remove it from each.

- [ ] **Step 9: Update the existing scenario tests**

  Each test calls a builder and asserts on the resulting flagged set. Change every call site from `let roots = build_x(dir.path());` to:

  ```rust
  let spec = build_x();
  let roots = materialize(&spec, dir.path());
  ```

  Tests to update: `mixed_forest_flags_the_expected_leaves`, `messy_shelf_flags_the_expected_leaves`, `clean_error_has_a_covered_root_and_an_uncreated_root`, `root_flagged_surfaces_the_root_itself`, `pre_marked_drops_covered_folders_and_keeps_click_targets`, `big_library_has_the_expected_flagged_count_and_anchor_states`, `big_library_generation_is_deterministic`. The flagged-set assertions stay byte-for-byte identical.

- [ ] **Step 10: Update consumers**

  Each consumer changes one line from `(scenario.build)(base)` to `materialize(&(scenario.spec)(), base)`.

  - `examples/explore.rs:161` — the harness call site.
  - `src/bin/demo.rs:55` — the demo binary entry.
  - `src/autosync.rs:496` — inside `#[cfg(test)]`. Add `use missing_ebooks::scenarios::materialize;` (or `crate::scenarios::materialize`, matching surrounding imports).
  - `src/service.rs:371` — inside `#[cfg(test)]`. Same import note.
  - `tests/sse.rs:44`, `:106`, `:161` — three integration test call sites. Add `use missing_ebooks::scenarios::materialize;` once at the top.
  - `tests/cache_render_byte_equal.rs:20` — one integration test call site. Same import.

  Verify with `rg "\.build\)" src/ tests/ examples/` after the edits: zero hits.

- [ ] **Step 11: Run the full suite**

  Run: `cargo test`
  Expected: every test passes. The flagged sets are unchanged because the spec materializes the same files in the same places.

- [ ] **Step 12: Run clippy and doc**

  Run: `cargo clippy --all-targets -- -D warnings && cargo doc --no-deps -D warnings`
  Expected: clean.

- [ ] **Step 13: Run the accent test if assets changed**

  This task does not touch assets, so skip `mise run test:accent`.

- [ ] **Step 14: Commit**

  ```bash
  git add src/scenarios.rs examples/explore.rs src/bin/demo.rs src/autosync.rs src/service.rs tests/sse.rs tests/cache_render_byte_equal.rs
  git commit -m "refactor(scenarios): rewrite builders as declarative ScenarioSpec

  Flips Scenario.build (fn(&Path) -> Vec<PathBuf>) to Scenario.spec
  (fn() -> ScenarioSpec) and rewrites every build_* to return a spec.
  Consumers call materialize(&(s.spec)(), base) to seed.

  No behavior change: the materialized on-disk layout matches the previous
  imperative touch() calls byte-for-byte, so every existing flagged-set
  assertion passes unchanged."
  ```

---

## Task 3: Close out the architecture-review entry

Records candidate #5 as done in the review README.

**Files:**
- Modify: `.scratch/architecture-review-2026-06/README.md`

- [ ] **Step 1: Mark #5 done in the status table**

  Change the row from `| 5 | Declarative \`ScenarioSpec\` data | Worth exploring | open |` to `| 5 | Declarative \`ScenarioSpec\` data | Worth exploring | **done** (see \`.scratch/scenario-spec-data/\`) |`.

- [ ] **Step 2: Update the "Suggested next pick" paragraph**

  Note that #5 has shipped and #6 (speculative) is the only open candidate.

- [ ] **Step 3: Commit**

  ```bash
  git add .scratch/architecture-review-2026-06/README.md
  git commit -m "docs(arch-review): mark candidate #5 done"
  ```

---

## Self-review

**Spec coverage:**
- Data model (`MarkerKind`, `Entry`, `RootSpec`, `RootPlan`, `ScenarioSpec`): Task 1, Step 4.
- `materialize` + `write_entry`: Task 1, Step 5.
- Constructors: Task 1, Step 6.
- New tests (`materialize_writes_marker_files_with_leading_dot`, `materialize_returns_uncreated_paths_without_touching_disk`): Task 1, Steps 1 and 2.
- `Scenario.spec` rename: Task 2, Step 1.
- Six rewritten builders: Task 2, Steps 2-7.
- Eight consumer updates: Task 2, Step 10.
- Existing test updates: Task 2, Step 9.
- Non-goals (no `expected_flagged`, no curated-contract change, no `touch` visibility change, no ADR): respected.

**Placeholder scan:** no TBDs. Step 6 of Task 2 says "translate every `touch` call" instead of inlining all 64 calls; the existing source is the literal reference and the test verifies correctness. Acceptable: the substitution rule is mechanical and unambiguous.

**Type consistency:** `Scenario.spec: fn() -> ScenarioSpec` (Task 2 Step 1) matches the `build_x() -> ScenarioSpec` signatures (Task 2 Steps 2-7) and the consumer calls `materialize(&(s.spec)(), base)` (Task 2 Step 10). `materialize` takes `&ScenarioSpec` everywhere. `Entry::Audio` and `Entry::Ebook` both carry `name: String` and both go through the same `touch(&parent.join(name))` arm in `write_entry`. Constructor names match between definition (Task 1 Step 6) and use (Task 2 Steps 2-7).
