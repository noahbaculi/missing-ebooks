//! Synthetic library scenarios shared by the `explore` dev harness and the
//! public demo binary. Each builder seeds folders and files under a base
//! directory with `std::fs` and returns the library roots to configure, in
//! render order. Loose root audio surfaces the root itself (see
//! docs/adr/0005-library-root-itself-flaggable.md).

use std::path::{Path, PathBuf};

/// One catalog entry: a name, a one-line description, and the builder that seeds
/// it. The builder creates the library tree or trees under the given base
/// directory and returns the library roots to configure, in render order.
#[derive(Clone, Copy)]
pub struct Scenario {
    /// The name used to select this scenario on the command line or in config.
    pub name: &'static str,
    /// A one-line description shown in the catalog listing.
    pub description: &'static str,
    /// Seeds the scenario under `base` and returns its library roots.
    pub build: fn(&Path) -> Vec<PathBuf>,
}

/// The shipped scenarios, in listing order.
pub fn catalog() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "mixed-forest",
            description: "Flagship nested tree: containers, flagged leaves, query cleaning, ancestor coverage (single root)",
            build: build_mixed_forest,
        },
        Scenario {
            name: "clean-error",
            description: "Two roots side by side: one fully covered (Clean), one uncreated (Error)",
            build: build_clean_error,
        },
        Scenario {
            name: "root-flagged",
            description: "Loose audio in the root, so the root itself is flagged (single root)",
            build: build_root_flagged,
        },
        Scenario {
            name: "pre-marked",
            description: "Pre-existing markers hide covered folders; siblings stay click targets (single root)",
            build: build_pre_marked,
        },
        Scenario {
            name: "big-library",
            description: "~50 authors of varying size with mixed coverage and nesting, for scroll and layout testing at volume (single root)",
            build: build_big_library,
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
fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        mkdirs(parent);
    }
    std::fs::write(path, b"").expect("write scenario file");
}

// --- Scenario builders. Each one seeds a synthetic library and returns its
// roots. ---

/// Flagship nested tree exercising containers, flagged leaves, query cleaning,
/// and ancestor coverage under one root.
pub fn build_mixed_forest(base: &Path) -> Vec<PathBuf> {
    let root = base.join("Library");
    // Andy Weir: a flagged leaf whose "(Unabridged)" suffix is stripped from the
    // search query, plus a sibling covered by its own epub so it drops out.
    touch(&root.join("Andy Weir/Artemis (Unabridged)/01 - Artemis.mp3"));
    touch(&root.join("Andy Weir/The Martian/01 - The Martian.m4b"));
    touch(&root.join("Andy Weir/The Martian/The Martian.epub"));
    // Cixin Liu: a series-level epub covers the whole subtree, so the author is
    // absent entirely.
    touch(&root.join("Cixin Liu/Remembrance of Earth's Past/Remembrance of Earth's Past.epub"));
    touch(&root.join(
        "Cixin Liu/Remembrance of Earth's Past/1 - The Three-Body Problem/01 - The Three-Body Problem.mp3",
    ));
    touch(&root.join(
        "Cixin Liu/Remembrance of Earth's Past/2 - The Dark Forest/01 - The Dark Forest.mp3",
    ));
    // Brandon Sanderson: two flagged leaves under a nested series container; the
    // "[2007]" segment is stripped from the second one's search query.
    touch(&root.join(
        "Brandon Sanderson/The Mistborn Saga/Mistborn 01 - The Final Empire/01 - The Final Empire.m4b",
    ));
    touch(&root.join(
        "Brandon Sanderson/The Mistborn Saga/Mistborn 02 - The Well of Ascension [2007]/01 - The Well of Ascension.m4b",
    ));
    // Robin Hobb: the U+2019 right single quotation mark in the folder name
    // survives cleaning and is percent-encoded in the search href.
    touch(&root.join(
        "Robin Hobb/Farseer Trilogy/1 - Assassin\u{2019}s Apprentice/01 - Assassin\u{2019}s Apprentice.m4b",
    ));
    // Frank Herbert: a "Dune Chronicles" series container with a mix of states.
    // "Dune" stays flagged (and exercises the .flac extension); "Dune Messiah"
    // is hidden by an .ebook_elsewhere marker and "Children of Dune" by its own
    // .pdf, so the container surfaces showing only its one remaining gap.
    touch(&root.join("Frank Herbert/Dune Chronicles/Dune/01 - Dune.flac"));
    touch(&root.join("Frank Herbert/Dune Chronicles/Dune Messiah/01 - Dune Messiah.mp3"));
    touch(&root.join("Frank Herbert/Dune Chronicles/Dune Messiah/.ebook_elsewhere"));
    touch(&root.join("Frank Herbert/Dune Chronicles/Children of Dune/01 - Children of Dune.mp3"));
    touch(&root.join("Frank Herbert/Dune Chronicles/Children of Dune/Children of Dune.pdf"));
    // Terry Pratchett: a "Discworld" series container. "Terry Pratchett - Mort"
    // keeps its internal " - " hyphen verbatim in the cleaned query (an author
    // prefix is not a dangling separator). "Guards! Guards!" is covered by its
    // own .mobi, demonstrating a non-epub ebook format counts as coverage.
    touch(&root.join("Terry Pratchett/Discworld/Terry Pratchett - Mort/01 - Mort.mp3"));
    touch(&root.join("Terry Pratchett/Discworld/Guards! Guards!/01 - Guards! Guards!.m4b"));
    touch(&root.join("Terry Pratchett/Discworld/Guards! Guards!/Guards! Guards!.mobi"));
    // Ursula K. Le Guin: "The Left Hand of Darkness" is flagged and exercises
    // the .m4a extension. "The Dispossessed" carries its own .no_ebook marker,
    // so it drops out while its flagged sibling stays a click target.
    touch(
        &root
            .join("Ursula K. Le Guin/The Left Hand of Darkness/01 - The Left Hand of Darkness.m4a"),
    );
    touch(&root.join("Ursula K. Le Guin/The Dispossessed/01 - The Dispossessed.mp3"));
    touch(&root.join("Ursula K. Le Guin/The Dispossessed/.no_ebook"));
    // Neal Stephenson: the underscore and dot in "The_Diamond.Age" normalize to
    // spaces, so the cleaned search query is "The Diamond Age". The flagged path
    // itself keeps the raw folder name.
    touch(&root.join("Neal Stephenson/The_Diamond.Age/01 - The Diamond Age.mp3"));
    // Dan Simmons: the "{Deluxe Edition}" brace segment is stripped from the
    // query, leaving "Hyperion". The leaf also exercises the .opus extension.
    touch(&root.join("Dan Simmons/Hyperion {Deluxe Edition}/01 - Hyperion.opus"));
    // Isaac Asimov: two cleaning cases. "Foundation [Book 1 (Unabridged)]" has a
    // nested bracket segment stripped whole, leaving "Foundation". "- The Caves
    // of Steel -" has leading and trailing dashes trimmed, leaving "The Caves of
    // Steel".
    touch(&root.join("Isaac Asimov/Foundation [Book 1 (Unabridged)]/01 - Foundation.m4b"));
    touch(&root.join("Isaac Asimov/- The Caves of Steel -/01 - The Caves of Steel.mp3"));
    // Neil Gaiman: two non-epub coverage formats. "The Sandman" is covered by a
    // .cbz comic archive and "American Gods" by an .azw3, so both drop out.
    // "Neverwhere" has no ebook, so the author still appears with one flagged
    // book. (.cbr is intentionally omitted: it shares the identical detection
    // path as .cbz, so a second comic format would add a folder without meaning.)
    touch(&root.join("Neil Gaiman/The Sandman/01 - The Sandman.mp3"));
    touch(&root.join("Neil Gaiman/The Sandman/The Sandman.cbz"));
    touch(&root.join("Neil Gaiman/American Gods/01 - American Gods.m4b"));
    touch(&root.join("Neil Gaiman/American Gods/American Gods.azw3"));
    touch(&root.join("Neil Gaiman/Neverwhere/01 - Neverwhere.mp3"));
    // Octavia E. Butler: a plain flagged book (audio, no ebook) for lifelike
    // volume, with no special cleaning or coverage case.
    touch(&root.join("Octavia E. Butler/Kindred/01 - Kindred.mp3"));
    // Arthur C. Clarke: the one author that mixes two series containers with
    // loose standalone books, so its node aggregates gaps from three groupings
    // at once. Each grouping pairs a flagged book with a covered sibling: in
    // "Space Odyssey", "2001 A Space Odyssey" stays flagged while "2010 Odyssey
    // Two" is covered by its .epub; in "Rama", "Rendezvous with Rama" stays
    // flagged while "Rama II" is covered; among the standalones, "The Fountains
    // of Paradise" stays flagged while "The City and the Stars" is covered.
    touch(
        &root.join(
            "Arthur C. Clarke/Space Odyssey/2001 A Space Odyssey/01 - 2001 A Space Odyssey.mp3",
        ),
    );
    touch(&root.join("Arthur C. Clarke/Space Odyssey/2010 Odyssey Two/01 - 2010 Odyssey Two.m4b"));
    touch(&root.join("Arthur C. Clarke/Space Odyssey/2010 Odyssey Two/2010 Odyssey Two.epub"));
    touch(&root.join("Arthur C. Clarke/Rama/Rendezvous with Rama/01 - Rendezvous with Rama.m4b"));
    touch(&root.join("Arthur C. Clarke/Rama/Rama II/01 - Rama II.mp3"));
    touch(&root.join("Arthur C. Clarke/Rama/Rama II/Rama II.epub"));
    touch(
        &root.join("Arthur C. Clarke/The Fountains of Paradise/01 - The Fountains of Paradise.mp3"),
    );
    touch(&root.join("Arthur C. Clarke/The City and the Stars/01 - The City and the Stars.m4b"));
    touch(&root.join("Arthur C. Clarke/The City and the Stars/The City and the Stars.epub"));
    vec![root]
}

/// Two roots side by side: one fully covered (Clean), one never created (Error).
pub fn build_clean_error(base: &Path) -> Vec<PathBuf> {
    // Root 1: every audio folder has an ebook beside it, so the root is Clean.
    let covered = base.join("Covered Library");
    touch(&covered.join("Author/Book/01 - Book.mp3"));
    touch(&covered.join("Author/Book/Book.epub"));
    // Root 2: a path we hand to library_roots but never create on disk. It cannot
    // canonicalize, so the section renders "Could not scan this root" and the
    // server logs the skip warning.
    let missing = base.join("Missing Library");
    vec![covered, missing]
}

/// Loose audio directly in the root, so the root itself is the gap (ADR-0005).
pub fn build_root_flagged(base: &Path) -> Vec<PathBuf> {
    // Audio loose in the root with no author/book folder: the root itself is the
    // gap, surfaced as a single flagged node with rel_path "." (see ADR-0005).
    let root = base.join("Loose Audio");
    touch(&root.join("01 - Some Lecture.mp3"));
    touch(&root.join("02 - Some Lecture.mp3"));
    vec![root]
}

/// Pre-existing markers hide covered folders while sibling gaps stay actionable.
pub fn build_pre_marked(base: &Path) -> Vec<PathBuf> {
    let root = base.join("Marked Library");
    // Covered Book carries its own .no_ebook, so it is absent; Uncovered Book has
    // no marker and stays as a click target.
    touch(&root.join("Marked Author/Covered Book/01 - Covered Book.m4b"));
    touch(&root.join("Marked Author/Covered Book/.no_ebook"));
    touch(&root.join("Marked Author/Uncovered Book/01 - Uncovered Book.m4b"));
    // A series-level .ebook_elsewhere covers everything below it, so the whole
    // Elsewhere Series subtree is absent.
    touch(&root.join("Elsewhere Series/.ebook_elsewhere"));
    touch(&root.join("Elsewhere Series/Book A/01 - Book A.mp3"));
    // Plain Author has no markers, so Plain Book stays flagged.
    touch(&root.join("Plain Author/Plain Book/01 - Plain Book.m4b"));
    vec![root]
}

/// About fifty authors of varying size and nesting, for layout testing at volume.
pub fn build_big_library(base: &Path) -> Vec<PathBuf> {
    let root = base.join("Audiobooks");

    // Name pools. Ten first names by seven last names give 70 unique
    // combinations; the loop uses the first 50 by index, so every author name is
    // distinct. The two title pools have periods 8 and 9, both larger than the
    // biggest author's 12 books, so titles never collide within one author.
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

    // The scrollable bulk: 50 authors, each with 1..=12 books, with coverage
    // assigned by a running book index so it spreads through the tree instead of
    // clustering. Every fifth book is covered by its own epub and every seventh
    // that is not already covered carries a marker, so both drop out of the
    // rendered tree while the rest stay flagged.
    let mut g: usize = 0;
    for a in 0..50usize {
        let author = format!("{} {}", FIRST_NAMES[a % 10], LAST_NAMES[(a / 10) % 7]);
        let books = 1 + (a % 12);
        // Every sixth author nests its books under a series container, so the
        // tree gains depth as well as breadth.
        let series = if a.is_multiple_of(6) {
            Some(format!("{} Cycle", LAST_NAMES[(a / 10) % 7]))
        } else {
            None
        };
        for b in 0..books {
            let title = format!(
                "{} {}",
                TITLE_LEFT[(a + b) % 8],
                TITLE_RIGHT[(a * 2 + b) % 9]
            );
            let book_dir = match &series {
                Some(series) => root.join(&author).join(series).join(&title),
                None => root.join(&author).join(&title),
            };
            touch(&book_dir.join(format!("01 - {title}.{}", AUDIO_EXT[g % 3])));
            if g.is_multiple_of(5) {
                // Covered: an ebook beside the audio, so the scanner drops it.
                touch(&book_dir.join(format!("{title}.epub")));
            } else if g.is_multiple_of(7) {
                // Pre-marked: alternate the two marker kinds for variety.
                let marker = if g.is_multiple_of(2) {
                    ".no_ebook"
                } else {
                    ".ebook_elsewhere"
                };
                touch(&book_dir.join(marker));
            }
            g += 1;
        }
    }

    // Fixed-name anchors. Stable names let the tests assert specific states
    // without reconstructing generated names, and they add a few deliberate
    // layout cases to the page.

    // A plainly flagged folder: audio, no ebook.
    touch(&root.join("Flagged Anchor/A Plain Flagged Book/01 - track.mp3"));

    // Covered by an in-folder epub, so it is absent from the tree.
    touch(&root.join("Covered Anchor/A Covered Book/01 - track.mp3"));
    touch(&root.join("Covered Anchor/A Covered Book/A Covered Book.epub"));

    // Covered by an in-folder marker, so it is absent while siblings would stay.
    touch(&root.join("Marked Anchor/A Pre-Marked Book/01 - track.m4b"));
    touch(&root.join("Marked Anchor/A Pre-Marked Book/.no_ebook"));

    // Ancestor coverage: one marker at the collection level hides the whole
    // subtree, so neither book is flagged.
    touch(&root.join("Ancestor-Covered Collection/.ebook_elsewhere"));
    touch(&root.join("Ancestor-Covered Collection/Book One/01 - track.mp3"));
    touch(&root.join("Ancestor-Covered Collection/Book Two/01 - track.mp3"));

    // A very long author and book name, to see how a wide row wraps.
    touch(&root.join(
        "A Very Long Author Name That Keeps Going For Layout Testing/\
An Equally Long Book Title That Should Wrap Across The Line When The Window Is Narrow/01 - track.mp3",
    ));

    // A deeply nested, non-ASCII case: accents and a U+2019 right single quote
    // that survives query cleaning and is percent-encoded in the search href.
    touch(&root.join(
        "\u{c9}mile R\u{ed}os/The Collected Works/Inner Series/Assassin\u{2019}s Apprentice (Unabridged)/01 - track.m4b",
    ));

    vec![root]
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    use crate::config::Config;
    use crate::scanner::{self, ScanSettings};

    /// The set of flagged folders the production scanner reports for a seeded
    /// root, as `/`-joined relative paths.
    fn flagged(root: &Path) -> BTreeSet<String> {
        let settings = ScanSettings::compile(Config::default().scan_inputs()).unwrap();
        scanner::scan(root, &settings)
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn mixed_forest_flags_the_expected_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let roots = build_mixed_forest(dir.path());
        assert_eq!(roots.len(), 1);
        let want: BTreeSet<String> = [
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
        .map(|s| s.to_string())
        .collect();
        assert_eq!(flagged(&roots[0]), want);
    }

    #[test]
    fn clean_error_has_a_covered_root_and_an_uncreated_root() {
        let dir = tempfile::tempdir().unwrap();
        let roots = build_clean_error(dir.path());
        assert_eq!(roots.len(), 2);
        // Root 1 exists and is fully covered, so the scanner reports no gaps.
        assert!(flagged(&roots[0]).is_empty());
        assert!(roots[0].is_dir());
        // Root 2 is intentionally never created: it cannot canonicalize, which is
        // what drives the Error state in the UI.
        assert!(!roots[1].exists());
    }

    #[test]
    fn root_flagged_surfaces_the_root_itself() {
        let dir = tempfile::tempdir().unwrap();
        let roots = build_root_flagged(dir.path());
        assert_eq!(roots.len(), 1);
        // Loose audio in the root is reported as the empty relative path (ADR-0005).
        assert_eq!(flagged(&roots[0]), BTreeSet::from(["".to_string()]));
    }

    #[test]
    fn pre_marked_drops_covered_folders_and_keeps_click_targets() {
        let dir = tempfile::tempdir().unwrap();
        let roots = build_pre_marked(dir.path());
        assert_eq!(roots.len(), 1);
        let want: BTreeSet<String> = ["Marked Author/Uncovered Book", "Plain Author/Plain Book"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(flagged(&roots[0]), want);
    }

    #[test]
    fn big_library_has_the_expected_flagged_count_and_anchor_states() {
        let dir = tempfile::tempdir().unwrap();
        let roots = build_big_library(dir.path());
        assert_eq!(roots.len(), 1);
        assert!(roots[0].is_dir());

        let flagged = flagged(&roots[0]);

        // The 216 flagged books from the bulk loop plus the three flagged
        // anchors. See the plan/builder for the coverage cadence behind this.
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
        let first_roots = build_big_library(first.path());
        let second_roots = build_big_library(second.path());
        // Two independent builds seed identical flagged sets, so the harness shows
        // the same tree on every launch.
        assert_eq!(flagged(&first_roots[0]), flagged(&second_roots[0]));
    }

    #[test]
    fn catalog_lists_all_five_scenarios() {
        let names: Vec<&str> = catalog().iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "mixed-forest",
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
}
