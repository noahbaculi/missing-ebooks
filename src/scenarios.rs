//! Synthetic library scenarios shared by the `explore` dev harness and the
//! public demo binary. Each builder seeds folders and files under a base
//! directory with `std::fs` and returns the library roots to configure, in
//! render order. Loose root audio surfaces the root itself (see
//! docs/adr/0005-library-root-itself-flaggable.md).

use std::path::{Path, PathBuf};

/// Marker file kind a scenario can drop into a folder. The dot prefix lives
/// in `materialize`, so a missing dot is unrepresentable.
pub enum MarkerKind {
    /// `.no_ebook`: this folder has audio but no ebook is expected.
    NoEbook,
    /// `.ebook_elsewhere`: the ebook lives in another library root.
    EbookElsewhere,
}

/// One node in a `ScenarioSpec` tree.
pub enum Entry {
    /// A subdirectory containing further entries.
    Folder {
        /// Folder name (one path component).
        name: String,
        /// Children written under this folder.
        items: Vec<Entry>,
    },
    /// Full audio filename including extension, e.g. `"01 - Dune.mp3"`.
    Audio {
        /// Audio filename written into the parent folder.
        name: String,
    },
    /// Full ebook filename including extension, e.g. `"Dune.epub"`.
    Ebook {
        /// Ebook filename written into the parent folder.
        name: String,
    },
    /// Marker file (`.no_ebook` or `.ebook_elsewhere`) in the parent folder.
    Marker(MarkerKind),
}

/// A library root that `materialize` seeds under `base`.
pub struct RootSpec {
    /// Root folder name, joined under `base`.
    pub name: String,
    /// Top-level entries written under this root.
    pub items: Vec<Entry>,
}

/// One root in a scenario. `Uncreated` reserves the path without touching
/// disk, so canonicalization fails and the section renders Error.
pub enum RootPlan {
    /// A root that `materialize` creates on disk.
    Created(RootSpec),
    /// A root whose path is returned but never created on disk.
    Uncreated {
        /// Root folder name, joined under `base` without being created.
        name: String,
    },
}

/// Declarative description of a synthetic library, walked by `materialize`.
pub struct ScenarioSpec {
    /// Roots in render order.
    pub roots: Vec<RootPlan>,
}

/// One catalog entry: a name, a one-line description, and the spec describing
/// its library shape.
#[derive(Clone, Copy)]
pub struct Scenario {
    /// The name used to select this scenario on the command line or in config.
    pub name: &'static str,
    /// A one-line description shown in the catalog listing.
    pub description: &'static str,
    /// Returns the scenario's library spec. Run `materialize(&spec, base)` to
    /// seed it.
    pub spec: fn() -> ScenarioSpec,
}

/// The shipped scenarios, in listing order.
pub fn catalog() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "mixed-forest",
            description: "Flagship tree across three roots: a showcase forest, a smaller forest with cross-root .ebook_elsewhere markers, and a fully covered Clean root",
            spec: build_mixed_forest,
        },
        Scenario {
            name: "messy-shelf",
            description: "Heterogeneous depth: standalone books, author-only and series-only folders, a half-sorted author, a dumping folder, beside one meticulous author>series>book pocket (single root)",
            spec: build_messy_shelf,
        },
        Scenario {
            name: "clean-error",
            description: "Two roots side by side: one fully covered (Clean), one uncreated (Error)",
            spec: build_clean_error,
        },
        Scenario {
            name: "root-flagged",
            description: "Loose audio in the root, so the root itself is flagged (single root)",
            spec: build_root_flagged,
        },
        Scenario {
            name: "pre-marked",
            description: "Pre-existing markers hide covered folders; siblings stay click targets (single root)",
            spec: build_pre_marked,
        },
        Scenario {
            name: "big-library",
            description: "~50 authors of varying size with mixed coverage and nesting, for scroll and layout testing at volume (single root)",
            spec: build_big_library,
        },
    ]
}

/// Resolve a scenario by exact name.
pub fn find_scenario(name: &str) -> Option<Scenario> {
    catalog().into_iter().find(|scenario| scenario.name == name)
}

/// Create `dir` and every parent. Panics on failure: this seeds a fresh temp
/// directory, so a failure here is a bug, not a runtime condition.
fn mkdirs(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create scenario directory");
}

/// Create an empty file at `path`, creating its parents first.
pub(crate) fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        mkdirs(parent);
    }
    std::fs::write(path, b"").expect("write scenario file");
}

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

fn folder(name: &str, items: Vec<Entry>) -> Entry {
    Entry::Folder {
        name: name.into(),
        items,
    }
}

fn audio(name: &str) -> Entry {
    Entry::Audio { name: name.into() }
}

fn ebook(name: &str) -> Entry {
    Entry::Ebook { name: name.into() }
}

fn no_ebook() -> Entry {
    Entry::Marker(MarkerKind::NoEbook)
}

fn elsewhere() -> Entry {
    Entry::Marker(MarkerKind::EbookElsewhere)
}

fn root(name: &str, items: Vec<Entry>) -> RootPlan {
    RootPlan::Created(RootSpec {
        name: name.into(),
        items,
    })
}

fn uncreated(name: &str) -> RootPlan {
    RootPlan::Uncreated { name: name.into() }
}

// Scenario builders. Each one returns a declarative `ScenarioSpec` that
// `materialize` walks to seed the synthetic library.

/// Flagship nested tree across three roots: the showcase `Library`, a smaller
/// `External Library` whose duplicated authors carry cross-root `.ebook_elsewhere`
/// markers, and a fully covered `Complete Library` that renders Clean.
fn build_mixed_forest() -> ScenarioSpec {
    ScenarioSpec {
        roots: vec![
            root(
                "Library",
                vec![
                    // Andy Weir: a flagged leaf whose "(Unabridged)" suffix is
                    // stripped from the search query, plus a sibling covered by
                    // its own epub so it drops out.
                    folder(
                        "Andy Weir",
                        vec![
                            folder("Artemis (Unabridged)", vec![audio("01 - Artemis.mp3")]),
                            folder(
                                "The Martian",
                                vec![audio("01 - The Martian.m4b"), ebook("The Martian.epub")],
                            ),
                        ],
                    ),
                    // Cixin Liu: a series-level epub covers the whole subtree,
                    // so the author is absent entirely.
                    folder(
                        "Cixin Liu",
                        vec![folder(
                            "Remembrance of Earth's Past",
                            vec![
                                ebook("Remembrance of Earth's Past.epub"),
                                folder(
                                    "1 - The Three-Body Problem",
                                    vec![audio("01 - The Three-Body Problem.mp3")],
                                ),
                                folder(
                                    "2 - The Dark Forest",
                                    vec![audio("01 - The Dark Forest.mp3")],
                                ),
                            ],
                        )],
                    ),
                    // Brandon Sanderson: two flagged leaves under a nested
                    // series container. The "[2007]" segment is stripped from
                    // the second one's search query.
                    folder(
                        "Brandon Sanderson",
                        vec![folder(
                            "The Mistborn Saga",
                            vec![
                                folder(
                                    "Mistborn 01 - The Final Empire",
                                    vec![audio("01 - The Final Empire.m4b")],
                                ),
                                folder(
                                    "Mistborn 02 - The Well of Ascension [2007]",
                                    vec![audio("01 - The Well of Ascension.m4b")],
                                ),
                            ],
                        )],
                    ),
                    // Robin Hobb: the U+2019 right single quotation mark in the
                    // folder name survives cleaning and is percent-encoded in
                    // the search href.
                    folder(
                        "Robin Hobb",
                        vec![folder(
                            "Farseer Trilogy",
                            vec![folder(
                                "1 - Assassin\u{2019}s Apprentice",
                                vec![audio("01 - Assassin\u{2019}s Apprentice.m4b")],
                            )],
                        )],
                    ),
                    // Frank Herbert: a "Dune Chronicles" container with a mix
                    // of states. "Dune" stays flagged (and exercises .flac),
                    // "Dune Messiah" is hidden by an .ebook_elsewhere marker,
                    // and "Children of Dune" by its own .pdf, so the container
                    // surfaces only its one remaining gap.
                    folder(
                        "Frank Herbert",
                        vec![folder(
                            "Dune Chronicles",
                            vec![
                                folder("Dune", vec![audio("01 - Dune.flac")]),
                                folder(
                                    "Dune Messiah",
                                    vec![audio("01 - Dune Messiah.mp3"), elsewhere()],
                                ),
                                folder(
                                    "Children of Dune",
                                    vec![
                                        audio("01 - Children of Dune.mp3"),
                                        ebook("Children of Dune.pdf"),
                                    ],
                                ),
                            ],
                        )],
                    ),
                    // Terry Pratchett: a "Discworld" container. "Terry
                    // Pratchett - Mort" keeps its internal " - " verbatim in
                    // the cleaned query (an author prefix is not a dangling
                    // separator). "Guards! Guards!" is covered by its own
                    // .mobi, so a non-epub format counts as coverage.
                    folder(
                        "Terry Pratchett",
                        vec![folder(
                            "Discworld",
                            vec![
                                folder("Terry Pratchett - Mort", vec![audio("01 - Mort.mp3")]),
                                folder(
                                    "Guards! Guards!",
                                    vec![
                                        audio("01 - Guards! Guards!.m4b"),
                                        ebook("Guards! Guards!.mobi"),
                                    ],
                                ),
                            ],
                        )],
                    ),
                    // Ursula K. Le Guin: "The Left Hand of Darkness" is flagged
                    // and exercises the .m4a extension. "The Dispossessed"
                    // carries its own .no_ebook marker, so it drops out while
                    // its flagged sibling stays a click target.
                    folder(
                        "Ursula K. Le Guin",
                        vec![
                            folder(
                                "The Left Hand of Darkness",
                                vec![audio("01 - The Left Hand of Darkness.m4a")],
                            ),
                            folder(
                                "The Dispossessed",
                                vec![audio("01 - The Dispossessed.mp3"), no_ebook()],
                            ),
                        ],
                    ),
                    // Neal Stephenson: the underscore and dot in
                    // "The_Diamond.Age" normalize to spaces, so the cleaned
                    // search query is "The Diamond Age". The flagged path
                    // itself keeps the raw folder name.
                    folder(
                        "Neal Stephenson",
                        vec![folder(
                            "The_Diamond.Age",
                            vec![audio("01 - The Diamond Age.mp3")],
                        )],
                    ),
                    // Dan Simmons: the "{Deluxe Edition}" brace segment is
                    // stripped from the query, leaving "Hyperion". The leaf
                    // also exercises the .opus extension.
                    folder(
                        "Dan Simmons",
                        vec![folder(
                            "Hyperion {Deluxe Edition}",
                            vec![audio("01 - Hyperion.opus")],
                        )],
                    ),
                    // Isaac Asimov: two cleaning cases. "Foundation [Book 1
                    // (Unabridged)]" has a nested bracket segment stripped
                    // whole, leaving "Foundation". "- The Caves of Steel -"
                    // has leading and trailing dashes trimmed, leaving "The
                    // Caves of Steel".
                    folder(
                        "Isaac Asimov",
                        vec![
                            folder(
                                "Foundation [Book 1 (Unabridged)]",
                                vec![audio("01 - Foundation.m4b")],
                            ),
                            folder(
                                "- The Caves of Steel -",
                                vec![audio("01 - The Caves of Steel.mp3")],
                            ),
                        ],
                    ),
                    // Neil Gaiman: two non-epub coverage formats. "The
                    // Sandman" is covered by a .cbz comic archive and
                    // "American Gods" by an .azw3, so both drop out.
                    // "Neverwhere" has no ebook, so the author still appears
                    // with one flagged book. (.cbr is intentionally omitted:
                    // it shares the identical detection path as .cbz, so a
                    // second comic format would add a folder without meaning.)
                    folder(
                        "Neil Gaiman",
                        vec![
                            folder(
                                "The Sandman",
                                vec![audio("01 - The Sandman.mp3"), ebook("The Sandman.cbz")],
                            ),
                            folder(
                                "American Gods",
                                vec![audio("01 - American Gods.m4b"), ebook("American Gods.azw3")],
                            ),
                            folder("Neverwhere", vec![audio("01 - Neverwhere.mp3")]),
                        ],
                    ),
                    // Octavia E. Butler: a plain flagged book (audio, no
                    // ebook) for lifelike volume, with no special cleaning or
                    // coverage case.
                    folder(
                        "Octavia E. Butler",
                        vec![folder("Kindred", vec![audio("01 - Kindred.mp3")])],
                    ),
                    // Arthur C. Clarke: the one author mixing two series
                    // containers with loose standalone books, so its node
                    // aggregates gaps from three groupings. Each pairs a
                    // flagged book with a covered sibling. "2001 A Space
                    // Odyssey" flagged, "2010 Odyssey Two" covered by .epub
                    // in "Space Odyssey". "Rendezvous with Rama" flagged,
                    // "Rama II" covered in "Rama". "The Fountains of
                    // Paradise" flagged, "The City and the Stars" covered
                    // among the standalones.
                    folder(
                        "Arthur C. Clarke",
                        vec![
                            folder(
                                "Space Odyssey",
                                vec![
                                    folder(
                                        "2001 A Space Odyssey",
                                        vec![audio("01 - 2001 A Space Odyssey.mp3")],
                                    ),
                                    folder(
                                        "2010 Odyssey Two",
                                        vec![
                                            audio("01 - 2010 Odyssey Two.m4b"),
                                            ebook("2010 Odyssey Two.epub"),
                                        ],
                                    ),
                                ],
                            ),
                            folder(
                                "Rama",
                                vec![
                                    folder(
                                        "Rendezvous with Rama",
                                        vec![audio("01 - Rendezvous with Rama.m4b")],
                                    ),
                                    folder(
                                        "Rama II",
                                        vec![audio("01 - Rama II.mp3"), ebook("Rama II.epub")],
                                    ),
                                ],
                            ),
                            folder(
                                "The Fountains of Paradise",
                                vec![audio("01 - The Fountains of Paradise.mp3")],
                            ),
                            folder(
                                "The City and the Stars",
                                vec![
                                    audio("01 - The City and the Stars.m4b"),
                                    ebook("The City and the Stars.epub"),
                                ],
                            ),
                        ],
                    ),
                ],
            ),
            // Root 2: a second, smaller mixed forest on another drive. Five
            // new authors carry their own gaps. Four authors duplicate root 1,
            // modeling the same author on a second drive: three carry an
            // .ebook_elsewhere marker (the ebook lives in the main library),
            // so they resolve here while staying flagged in root 1, and
            // Octavia Butler's Kindred has no marker, so the same gap surfaces
            // in both roots.
            root(
                "External Library",
                vec![
                    // Becky Chambers: a "Wayfarers" container. "The Long Way
                    // to a Small, Angry Planet" stays flagged. "A Closed and
                    // Common Orbit" is covered by its own epub, so the
                    // container surfaces only its one remaining gap.
                    folder(
                        "Becky Chambers",
                        vec![folder(
                            "Wayfarers",
                            vec![
                                folder(
                                    "The Long Way to a Small, Angry Planet",
                                    vec![audio("01 - The Long Way to a Small, Angry Planet.mp3")],
                                ),
                                folder(
                                    "A Closed and Common Orbit",
                                    vec![
                                        audio("01 - A Closed and Common Orbit.m4b"),
                                        ebook("A Closed and Common Orbit.epub"),
                                    ],
                                ),
                            ],
                        )],
                    ),
                    // N.K. Jemisin: "The Fifth Season" stays flagged. "The
                    // Obelisk Gate" is covered by its own epub, so the author
                    // surfaces with one gap.
                    folder(
                        "N.K. Jemisin",
                        vec![
                            folder("The Fifth Season", vec![audio("01 - The Fifth Season.m4b")]),
                            folder(
                                "The Obelisk Gate",
                                vec![
                                    audio("01 - The Obelisk Gate.mp3"),
                                    ebook("The Obelisk Gate.epub"),
                                ],
                            ),
                        ],
                    ),
                    // Ann Leckie and Ted Chiang: a plain flagged book each, no
                    // coverage case.
                    folder(
                        "Ann Leckie",
                        vec![folder(
                            "Ancillary Justice",
                            vec![audio("01 - Ancillary Justice.mp3")],
                        )],
                    ),
                    folder(
                        "Ted Chiang",
                        vec![folder("Exhalation", vec![audio("01 - Exhalation.m4a")])],
                    ),
                    // Martha Wells: a "Murderbot Diaries" series container
                    // with two flagged books.
                    folder(
                        "Martha Wells",
                        vec![folder(
                            "The Murderbot Diaries",
                            vec![
                                folder("All Systems Red", vec![audio("01 - All Systems Red.mp3")]),
                                folder(
                                    "Artificial Condition",
                                    vec![audio("01 - Artificial Condition.m4b")],
                                ),
                            ],
                        )],
                    ),
                    // Four duplicates of root 1 authors. Three carry
                    // .ebook_elsewhere, so they resolve here while their root
                    // 1 copies stay flagged. Kindred has no marker, so it
                    // stays flagged in both roots.
                    folder(
                        "Andy Weir",
                        vec![folder(
                            "Artemis",
                            vec![audio("01 - Artemis.mp3"), elsewhere()],
                        )],
                    ),
                    folder(
                        "Brandon Sanderson",
                        vec![folder(
                            "The Mistborn Saga",
                            vec![folder(
                                "Mistborn 01 - The Final Empire",
                                vec![audio("01 - The Final Empire.m4b"), elsewhere()],
                            )],
                        )],
                    ),
                    folder(
                        "Isaac Asimov",
                        vec![folder(
                            "Foundation",
                            vec![audio("01 - Foundation.m4b"), elsewhere()],
                        )],
                    ),
                    folder(
                        "Octavia E. Butler",
                        vec![folder("Kindred", vec![audio("01 - Kindred.mp3")])],
                    ),
                ],
            ),
            // Root 3: a fully covered library. Every audio folder has its own
            // ebook beside it, so the scanner reports no gaps and the section
            // renders Clean.
            root(
                "Complete Library",
                vec![
                    folder(
                        "Adrian Tchaikovsky",
                        vec![folder(
                            "Children of Time",
                            vec![
                                audio("01 - Children of Time.m4b"),
                                ebook("Children of Time.epub"),
                            ],
                        )],
                    ),
                    folder(
                        "Kim Stanley Robinson",
                        vec![folder(
                            "Red Mars",
                            vec![audio("01 - Red Mars.mp3"), ebook("Red Mars.epub")],
                        )],
                    ),
                    folder(
                        "Connie Willis",
                        vec![folder(
                            "Doomsday Book",
                            vec![audio("01 - Doomsday Book.m4a"), ebook("Doomsday Book.epub")],
                        )],
                    ),
                ],
            ),
        ],
    }
}

/// A library a careless owner never tidied: standalone books, author-only and
/// series-only folders, and a dumping folder, beside one meticulous
/// author>series>book pocket. Flagged folders land at depths 1, 2, and 3 in one
/// tree, which the clean `build_mixed_forest` hierarchy never produces.
fn build_messy_shelf() -> ScenarioSpec {
    ScenarioSpec {
        roots: vec![root(
            "Audiobooks",
            vec![
                // Standalone books with no author folder above them, each
                // flagged at the root's first level.
                folder("The Hobbit", vec![audio("01 - The Hobbit.mp3")]),
                folder("Neuromancer", vec![audio("01 - Neuromancer.m4b")]),
                // Project Hail Mary is an Andy Weir book left loose at the top
                // instead of under the "Andy Weir" folder below: the same
                // author filed two ways.
                folder(
                    "Project Hail Mary",
                    vec![audio("01 - Project Hail Mary.mp3")],
                ),
                // Dune carries its own epub, so it is covered and drops out of
                // the tree.
                folder("Dune", vec![audio("01 - Dune.mp3"), ebook("Dune.epub")]),
                // Author folders that hold audio directly, with no book
                // subfolder, so the author folder itself is the flagged leaf.
                folder("Stephen King", vec![audio("01 - The Gunslinger.mp3")]),
                folder("Neil Gaiman", vec![audio("01 - Coraline.m4a")]),
                // A half-sorted author: one book loose in the author folder,
                // another in its own subfolder, so the author folder and its
                // book both flag.
                folder(
                    "Terry Pratchett",
                    vec![
                        audio("01 - The Colour of Magic.mp3"),
                        folder("Going Postal", vec![audio("01 - Going Postal.m4b")]),
                    ],
                ),
                // Dumping containers whose names a tidy library would not use.
                // The files keep descriptive names even though the folders do
                // not, so the file display can recover what the folder name
                // hides.
                folder(
                    "To Sort",
                    vec![
                        folder(
                            "Some Download",
                            vec![audio("Becky Chambers - Record of a Spaceborn Few.mp3")],
                        ),
                        folder(
                            "Another Rip",
                            vec![audio("Martha Wells - Network Effect.m4b")],
                        ),
                        // A pile that itself grew a subfolder, so a gap sits
                        // three levels down.
                        folder(
                            "Box Set",
                            vec![folder("Disc 1", vec![audio("Title Sequence 01.mp3")])],
                        ),
                    ],
                ),
                folder(
                    "Downloads",
                    vec![folder(
                        "Unknown Audiobook",
                        vec![audio("Ursula K. Le Guin - The Tombs of Atuan.mp3")],
                    )],
                ),
                // A normal author > book pair. Artemis stays flagged. The
                // Martian carries a .no_ebook marker, so it drops out while
                // its sibling stays.
                folder(
                    "Andy Weir",
                    vec![
                        folder("Artemis", vec![audio("01 - Artemis.mp3")]),
                        folder(
                            "The Martian",
                            vec![audio("01 - The Martian.m4b"), no_ebook()],
                        ),
                    ],
                ),
                folder(
                    "Ursula K. Le Guin",
                    vec![folder(
                        "The Left Hand of Darkness",
                        vec![audio("01 - The Left Hand of Darkness.mp3")],
                    )],
                ),
                // A series container with no author above it: the owner filed
                // the series but not the writer.
                folder(
                    "The Expanse",
                    vec![
                        folder("Leviathan Wakes", vec![audio("01 - Leviathan Wakes.mp3")]),
                        folder("Caliban's War", vec![audio("01 - Caliban's War.m4b")]),
                        folder("Abaddon's Gate", vec![audio("01 - Abaddon's Gate.mp3")]),
                    ],
                ),
                // Another series with no author above it, half-covered: The
                // Great Hunt is hidden by an .ebook_elsewhere marker, so the
                // container shows only its one remaining gap.
                folder(
                    "Wheel of Time",
                    vec![
                        folder(
                            "The Eye of the World",
                            vec![audio("01 - The Eye of the World.mp3")],
                        ),
                        folder(
                            "The Great Hunt",
                            vec![audio("01 - The Great Hunt.m4b"), elsewhere()],
                        ),
                    ],
                ),
                // The one meticulous pocket: a full author > series > book
                // hierarchy, one author with two series, so flagged leaves
                // reach depth 3.
                folder(
                    "Brandon Sanderson",
                    vec![
                        folder(
                            "The Stormlight Archive",
                            vec![
                                folder(
                                    "The Way of Kings",
                                    vec![audio("01 - The Way of Kings.m4b")],
                                ),
                                folder(
                                    "Words of Radiance",
                                    vec![audio("01 - Words of Radiance.mp3")],
                                ),
                            ],
                        ),
                        folder(
                            "Mistborn",
                            vec![folder(
                                "The Final Empire",
                                vec![audio("01 - The Final Empire.m4b")],
                            )],
                        ),
                    ],
                ),
            ],
        )],
    }
}

/// Two roots side by side: one fully covered (Clean), one never created (Error).
fn build_clean_error() -> ScenarioSpec {
    ScenarioSpec {
        roots: vec![
            // Every audio folder has an ebook beside it, so the root is Clean.
            root(
                "Covered Library",
                vec![folder(
                    "Author",
                    vec![folder(
                        "Book",
                        vec![audio("01 - Book.mp3"), ebook("Book.epub")],
                    )],
                )],
            ),
            // Path handed to `library_roots` but never created. It cannot
            // canonicalize, so the section renders "Could not scan this root"
            // and the server logs the skip warning.
            uncreated("Missing Library"),
        ],
    }
}

/// Loose audio directly in the root, so the root itself is the gap (ADR-0005).
fn build_root_flagged() -> ScenarioSpec {
    ScenarioSpec {
        roots: vec![
            // Audio loose in the root: the root itself is the gap, a single
            // flagged node with rel_path "." (see ADR-0005).
            root(
                "Loose Audio",
                vec![
                    audio("01 - Some Lecture.mp3"),
                    audio("02 - Some Lecture.mp3"),
                ],
            ),
        ],
    }
}

/// Pre-existing markers hide covered folders while sibling gaps stay actionable.
fn build_pre_marked() -> ScenarioSpec {
    ScenarioSpec {
        roots: vec![root(
            "Marked Library",
            vec![
                folder(
                    "Marked Author",
                    vec![
                        // Covered: carries `.no_ebook`, so it is absent.
                        folder(
                            "Covered Book",
                            vec![audio("01 - Covered Book.m4b"), no_ebook()],
                        ),
                        // No marker, so it stays a click target.
                        folder("Uncovered Book", vec![audio("01 - Uncovered Book.m4b")]),
                    ],
                ),
                // Series-level `.ebook_elsewhere` covers the whole subtree.
                folder(
                    "Elsewhere Series",
                    vec![
                        elsewhere(),
                        folder("Book A", vec![audio("01 - Book A.mp3")]),
                    ],
                ),
                // No markers, so Plain Book stays flagged.
                folder(
                    "Plain Author",
                    vec![folder("Plain Book", vec![audio("01 - Plain Book.m4b")])],
                ),
            ],
        )],
    }
}

/// About fifty authors of varying size and nesting, for layout testing at volume.
fn build_big_library() -> ScenarioSpec {
    // Name pools. Ten first names by seven last names give 70 unique combinations,
    // and the loop uses the first 50 by index, so every author name is distinct.
    // The two title pools have periods 8 and 9, both larger than the biggest
    // author's 12 books, so titles never collide within one author.
    const FIRST_NAMES: [&str; 10] = [
        "Ava", "Noah", "Mara", "Idris", "Lena", "Cole", "Priya", "Sten", "Yuki", "Rosa",
    ];
    const LAST_NAMES: [&str; 7] = [
        "Okafor",
        "Lindqvist",
        "Castellanos",
        "Nakamura",
        "Abernathy",
        "Delacroix",
        "Vandermeer",
    ];
    const TITLE_LEFT: [&str; 8] = [
        "The Hollow",
        "A Distant",
        "Iron",
        "The Last",
        "Crimson",
        "Pale",
        "The Silent",
        "Broken",
    ];
    const TITLE_RIGHT: [&str; 9] = [
        "Horizon", "Archive", "Tide", "Cipher", "Garden", "Engine", "Requiem", "Lantern",
        "Meridian",
    ];
    const AUDIO_EXT: [&str; 3] = ["mp3", "m4b", "m4a"];

    // Per-author folders accumulate here. Series-nested authors fold their
    // books under a series Folder before joining `authors`.
    let mut authors: Vec<Entry> = Vec::new();
    let mut g: usize = 0;

    // The scrollable bulk: 50 authors, each with 1..=12 books, with coverage
    // assigned by a running book index so it spreads through the tree instead
    // of clustering. Every fifth book is covered by its own epub and every
    // seventh that is not already covered carries a marker, so both drop out
    // of the rendered tree while the rest stay flagged.
    for a in 0..50usize {
        let author_name = format!("{} {}", FIRST_NAMES[a % 10], LAST_NAMES[(a / 10) % 7]);
        let book_count = 1 + (a % 12);
        // Every sixth author nests its books under a series container, so the
        // tree gains depth as well as breadth.
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
            let mut book_items: Vec<Entry> =
                vec![audio(&format!("01 - {title}.{}", AUDIO_EXT[g % 3]))];
            if g.is_multiple_of(5) {
                // Covered: an ebook beside the audio, so the scanner drops it.
                book_items.push(ebook(&format!("{title}.epub")));
            } else if g.is_multiple_of(7) {
                // Pre-marked: alternate the two marker kinds for variety.
                book_items.push(if g.is_multiple_of(2) {
                    no_ebook()
                } else {
                    elsewhere()
                });
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

    // Fixed-name anchors. Stable names let the tests assert specific states
    // without reconstructing generated names, and they add a few deliberate
    // layout cases to the page.

    // A plainly flagged folder: audio, no ebook.
    authors.push(folder(
        "Flagged Anchor",
        vec![folder(
            "A Plain Flagged Book",
            vec![audio("01 - track.mp3")],
        )],
    ));
    // Covered by an in-folder epub, so it is absent from the tree.
    authors.push(folder(
        "Covered Anchor",
        vec![folder(
            "A Covered Book",
            vec![audio("01 - track.mp3"), ebook("A Covered Book.epub")],
        )],
    ));
    // Covered by an in-folder marker, so it is absent while siblings would stay.
    authors.push(folder(
        "Marked Anchor",
        vec![folder(
            "A Pre-Marked Book",
            vec![audio("01 - track.m4b"), no_ebook()],
        )],
    ));
    // Ancestor coverage: one marker at the collection level hides the whole
    // subtree, so neither book is flagged.
    authors.push(folder(
        "Ancestor-Covered Collection",
        vec![
            elsewhere(),
            folder("Book One", vec![audio("01 - track.mp3")]),
            folder("Book Two", vec![audio("01 - track.mp3")]),
        ],
    ));
    // A very long author and book name, to see how a wide row wraps.
    authors.push(folder(
        "A Very Long Author Name That Keeps Going For Layout Testing",
        vec![folder(
            "An Equally Long Book Title That Should Wrap Across The Line When The Window Is Narrow",
            vec![audio("01 - track.mp3")],
        )],
    ));
    // A deeply nested, non-ASCII case: accents and a U+2019 right single quote
    // that survives query cleaning and is percent-encoded in the search href.
    authors.push(folder(
        "\u{c9}mile R\u{ed}os",
        vec![folder(
            "The Collected Works",
            vec![folder(
                "Inner Series",
                vec![folder(
                    "Assassin\u{2019}s Apprentice (Unabridged)",
                    vec![audio("01 - track.m4b")],
                )],
            )],
        )],
    ));

    ScenarioSpec {
        roots: vec![root("Audiobooks", authors)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    use crate::config::Config;
    use crate::scanner::{self, DirIndex, ScanSettings};

    /// The set of flagged folders the production scanner reports for a seeded
    /// root, as `/`-joined relative paths.
    fn flagged(root: &Path) -> BTreeSet<String> {
        let settings = ScanSettings::compile(Config::default().scan_inputs()).unwrap();
        scanner::scan_warm(root, &settings, &DirIndex::new())
            .0
            .iter()
            .filter(|f| f.directly_holds_audio && f.missing_ebook)
            .map(|f| f.rel_path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn mixed_forest_flags_the_expected_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let spec = build_mixed_forest();
        let roots = materialize(&spec, dir.path());
        assert_eq!(roots.len(), 3);

        // Root 0: the showcase forest.
        let want_library: BTreeSet<String> = [
            "Andy Weir/Artemis (Unabridged)",
            "Brandon Sanderson/The Mistborn Saga/Mistborn 01 - The Final Empire",
            "Brandon Sanderson/The Mistborn Saga/Mistborn 02 - The Well of Ascension [2007]",
            "Robin Hobb/Farseer Trilogy/1 - Assassin\u{2019}s Apprentice",
            "Frank Herbert/Dune Chronicles/Dune",
            "Terry Pratchett/Discworld/Terry Pratchett - Mort",
            "Ursula K. Le Guin/The Left Hand of Darkness",
            "Neal Stephenson/The_Diamond.Age",
            "Dan Simmons/Hyperion {Deluxe Edition}",
            "Isaac Asimov/Foundation [Book 1 (Unabridged)]",
            "Isaac Asimov/- The Caves of Steel -",
            "Neil Gaiman/Neverwhere",
            "Octavia E. Butler/Kindred",
            "Arthur C. Clarke/Space Odyssey/2001 A Space Odyssey",
            "Arthur C. Clarke/Rama/Rendezvous with Rama",
            "Arthur C. Clarke/The Fountains of Paradise",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(flagged(&roots[0]), want_library);

        // Root 1: External Library, a smaller forest. The three .ebook_elsewhere
        // duplicates resolve here. Octavia Butler's Kindred has no marker, so the
        // same gap surfaces in both roots.
        let want_external: BTreeSet<String> = [
            "Ann Leckie/Ancillary Justice",
            "Becky Chambers/Wayfarers/The Long Way to a Small, Angry Planet",
            "Martha Wells/The Murderbot Diaries/All Systems Red",
            "Martha Wells/The Murderbot Diaries/Artificial Condition",
            "N.K. Jemisin/The Fifth Season",
            "Octavia E. Butler/Kindred",
            "Ted Chiang/Exhalation",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(flagged(&roots[1]), want_external);

        // Root 2: Complete Library, fully covered, so no gaps.
        assert!(flagged(&roots[2]).is_empty());
        assert!(roots[2].is_dir());
    }

    #[test]
    fn messy_shelf_flags_the_expected_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let spec = build_messy_shelf();
        let roots = materialize(&spec, dir.path());
        assert_eq!(roots.len(), 1);
        let want: BTreeSet<String> = [
            "The Hobbit",
            "Neuromancer",
            "Project Hail Mary",
            "Stephen King",
            "Neil Gaiman",
            "Terry Pratchett",
            "Terry Pratchett/Going Postal",
            "To Sort/Some Download",
            "To Sort/Another Rip",
            "To Sort/Box Set/Disc 1",
            "Downloads/Unknown Audiobook",
            "Andy Weir/Artemis",
            "Ursula K. Le Guin/The Left Hand of Darkness",
            "The Expanse/Leviathan Wakes",
            "The Expanse/Caliban's War",
            "The Expanse/Abaddon's Gate",
            "Wheel of Time/The Eye of the World",
            "Brandon Sanderson/The Stormlight Archive/The Way of Kings",
            "Brandon Sanderson/The Stormlight Archive/Words of Radiance",
            "Brandon Sanderson/Mistborn/The Final Empire",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(flagged(&roots[0]), want);
    }

    #[test]
    fn clean_error_has_a_covered_root_and_an_uncreated_root() {
        let dir = tempfile::tempdir().unwrap();
        let spec = build_clean_error();
        let roots = materialize(&spec, dir.path());
        assert_eq!(roots.len(), 2);
        // Root 1 exists and is fully covered, so the scanner reports no gaps.
        assert!(flagged(&roots[0]).is_empty());
        assert!(roots[0].is_dir());
        // Root 2 is never created: it cannot canonicalize, which drives the Error
        // state in the UI.
        assert!(!roots[1].exists());
    }

    #[test]
    fn root_flagged_surfaces_the_root_itself() {
        let dir = tempfile::tempdir().unwrap();
        let spec = build_root_flagged();
        let roots = materialize(&spec, dir.path());
        assert_eq!(roots.len(), 1);
        // Loose audio in the root is reported as the empty relative path (ADR-0005).
        assert_eq!(flagged(&roots[0]), BTreeSet::from([String::new()]));
    }

    #[test]
    fn pre_marked_drops_covered_folders_and_keeps_click_targets() {
        let dir = tempfile::tempdir().unwrap();
        let spec = build_pre_marked();
        let roots = materialize(&spec, dir.path());
        assert_eq!(roots.len(), 1);
        let want: BTreeSet<String> = ["Marked Author/Uncovered Book", "Plain Author/Plain Book"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(flagged(&roots[0]), want);
    }

    #[test]
    fn big_library_has_the_expected_flagged_count_and_anchor_states() {
        let dir = tempfile::tempdir().unwrap();
        let spec = build_big_library();
        let roots = materialize(&spec, dir.path());
        assert_eq!(roots.len(), 1);
        assert!(roots[0].is_dir());

        let flagged = flagged(&roots[0]);

        // The 216 flagged books from the bulk loop plus the three flagged anchors.
        // See the builder for the coverage cadence.
        assert_eq!(flagged.len(), 219);

        // Fixed-name anchors pin specific coverage states.
        assert!(flagged.contains("Flagged Anchor/A Plain Flagged Book"));
        assert!(!flagged.contains("Covered Anchor/A Covered Book"));
        assert!(!flagged.contains("Marked Anchor/A Pre-Marked Book"));
        // An ancestor marker hides the whole collection subtree.
        assert!(!flagged.contains("Ancestor-Covered Collection/Book One"));
    }

    #[test]
    fn big_library_generation_is_deterministic() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_spec = build_big_library();
        let first_roots = materialize(&first_spec, first.path());
        let second_spec = build_big_library();
        let second_roots = materialize(&second_spec, second.path());
        // Two independent builds seed identical flagged sets, so the harness shows
        // the same tree on every launch.
        assert_eq!(flagged(&first_roots[0]), flagged(&second_roots[0]));
    }

    #[test]
    fn catalog_lists_all_six_scenarios() {
        let names: Vec<&str> = catalog().iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "mixed-forest",
                "messy-shelf",
                "clean-error",
                "root-flagged",
                "pre-marked",
                "big-library"
            ]
        );
    }

    #[test]
    fn find_scenario_matches_by_name_and_rejects_unknown() {
        assert!(find_scenario("pre-marked").is_some());
        assert!(find_scenario("does-not-exist").is_none());
    }

    #[test]
    fn materialize_writes_marker_files_with_leading_dot() {
        let dir = tempfile::tempdir().unwrap();
        let spec = ScenarioSpec {
            roots: vec![root(
                "R",
                vec![folder("F", vec![audio("a.mp3"), no_ebook(), elsewhere()])],
            )],
        };
        materialize(&spec, dir.path());
        assert!(dir.path().join("R/F/.no_ebook").is_file());
        assert!(dir.path().join("R/F/.ebook_elsewhere").is_file());
        assert!(dir.path().join("R/F/a.mp3").is_file());
    }

    #[test]
    fn materialize_returns_uncreated_paths_without_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let spec = ScenarioSpec {
            roots: vec![uncreated("Missing")],
        };
        let roots = materialize(&spec, dir.path());
        assert_eq!(roots, vec![dir.path().join("Missing")]);
        assert!(!roots[0].exists());
    }
}
