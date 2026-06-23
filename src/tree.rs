//! Build the forest of nodes for one root from the flat `Vec<ScannedFolder>`
//! the scanner produces. Each folder carries its own facts. Intermediate
//! containers absent from the input get a safe-neutral placeholder
//! (`directly_holds_audio = false, missing_ebook = true`). For a gaps-filtered
//! input the placeholder reproduces a bare container above a flagged leaf. For
//! a show-all input every folder appears, so each node's own entry overwrites
//! the placeholder. Siblings are ordered by case-insensitive natural sort, so
//! `Book 2` precedes `Book 10`. The empty relative path is the library root
//! itself (loose root audio, see ADR-0005): when it directly holds audio it
//! becomes a node named after the root with relative path `.`, pinned ahead of
//! the forest. The types derive `Serialize` for a future JSON API.

use std::path::Component;

use serde::Serialize;

/// One folder in a rendered tree. Two orthogonal facts describe it: whether it
/// directly holds audio, and whether it is missing an ebook (uncovered). The gap
/// the tool surfaces is the derived `needs_ebook()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    /// The folder's own name (its last path component).
    pub name: String,
    /// The folder's path relative to the library root, joined with `/`.
    pub rel_path: String,
    /// This folder directly contains at least one audio file.
    pub directly_holds_audio: bool,
    /// No ebook or marker covers it: none here and none in any ancestor up to the
    /// root. The inverse of CONTEXT.md's "covered".
    pub missing_ebook: bool,
    /// Child nodes, natural-sorted and case-insensitive.
    pub children: Vec<Node>,
    /// Ebook and marker filenames that physically sit in this folder. Mirrors
    /// `scanner::ScannedFolder::cover_files`. Empty in gaps-only.
    pub cover_files: Vec<String>,
    /// Audio filenames that physically sit in this folder, natural-sorted. Non-empty
    /// only where the folder directly holds audio; the file display reads it.
    pub audio_files: Vec<String>,
}

impl Node {
    /// A gap: this folder holds audio and is missing an ebook. Reproduces the old
    /// `flagged` value. CONTEXT.md's "flagged folder".
    #[must_use]
    pub fn needs_ebook(&self) -> bool {
        self.directly_holds_audio && self.missing_ebook
    }

    /// True when this node or any descendant is a gap. Drives the affordance rule:
    /// marker buttons and search links appear only where there is a gap to act on.
    #[must_use]
    pub fn has_gap_within(&self) -> bool {
        self.needs_ebook() || self.children.iter().any(Node::has_gap_within)
    }
}

/// Build the forest of top-level nodes for one root from the flat
/// `Vec<ScannedFolder>` the scanner produces. Every folder carries its own two
/// facts. Intermediate containers absent from the input get a placeholder
/// (`directly_holds_audio = false, missing_ebook = true`), the safe neutral. For
/// a gaps-filtered input (`reduce_to_flagged` output) the placeholder reproduces
/// today's pruned-walk behavior for the bare container above a flagged leaf. For
/// a show-all input every folder appears, so each node's own entry overwrites
/// the placeholder. `root_name` names the `.` node emitted when the root itself
/// directly holds audio (see ADR-0005). Siblings are natural-sorted.
#[must_use]
pub fn build(root_name: &str, folders: &[crate::scanner::ScannedFolder]) -> Vec<Node> {
    let mut roots: Vec<Node> = Vec::new();
    let mut root_entry: Option<&crate::scanner::ScannedFolder> = None;
    for folder in folders {
        let components: Vec<String> = folder
            .rel_path
            .components()
            .filter_map(|c| match c {
                Component::Normal(os) => Some(os.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if components.is_empty() {
            // The empty relative path is the library root itself (see ADR-0005).
            root_entry = Some(folder);
            continue;
        }
        insert_all(&mut roots, &components, "", folder);
    }
    sort_forest(&mut roots);
    if let Some(entry) = root_entry
        && entry.directly_holds_audio
    {
        // The root directly holds audio: surface it as a node, pinned ahead of
        // the author forest (ADR-0005). In show-all it shows even when covered.
        // In gaps the filter only keeps it when `missing_ebook` is also true, so
        // it appears exactly when it is a gap.
        roots.insert(
            0,
            Node {
                name: root_name.to_string(),
                rel_path: ".".to_string(),
                directly_holds_audio: true,
                missing_ebook: entry.missing_ebook,
                children: Vec::new(),
                cover_files: entry.cover_files.clone(),
                audio_files: entry.audio_files.clone(),
            },
        );
    }
    roots
}

fn insert_all(
    siblings: &mut Vec<Node>,
    components: &[String],
    parent_rel: &str,
    folder: &crate::scanner::ScannedFolder,
) {
    let Some((head, tail)) = components.split_first() else {
        return;
    };
    let rel_path = child_rel(parent_rel, head);
    let idx = match siblings.iter().position(|n| &n.name == head) {
        Some(i) => i,
        None => {
            siblings.push(Node {
                name: head.clone(),
                rel_path: rel_path.clone(),
                // Placeholder facts, overwritten when this folder's own entry is
                // processed. `scan` emits every folder, so that always happens.
                // A plain container is the safe neutral if it somehow were not.
                directly_holds_audio: false,
                missing_ebook: true,
                children: Vec::new(),
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            });
            siblings.len() - 1
        }
    };
    if tail.is_empty() {
        siblings[idx].directly_holds_audio = folder.directly_holds_audio;
        siblings[idx].missing_ebook = folder.missing_ebook;
        siblings[idx].cover_files = folder.cover_files.clone();
        siblings[idx].audio_files = folder.audio_files.clone();
    } else {
        insert_all(&mut siblings[idx].children, tail, &rel_path, folder);
    }
}

/// Join a parent's relative path with a child name, the way the scanner spells a
/// folder's path relative to its library root: the head alone at the top level,
/// `parent/head` below it.
fn child_rel(parent_rel: &str, head: &str) -> String {
    if parent_rel.is_empty() {
        head.to_string()
    } else {
        format!("{parent_rel}/{head}")
    }
}

fn sort_forest(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| lexical_sort::natural_lexical_cmp(&a.name, &b.name));
    for node in nodes {
        sort_forest(&mut node.children);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::ScannedFolder;
    use std::path::PathBuf;

    fn ff(rel: &str, audio: &[&str]) -> crate::scanner::ScannedFolder {
        crate::scanner::ScannedFolder {
            rel_path: PathBuf::from(rel),
            directly_holds_audio: true,
            missing_ebook: true,
            cover_files: Vec::new(),
            audio_files: audio.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn forest(paths: &[&str]) -> Vec<Node> {
        let owned: Vec<crate::scanner::ScannedFolder> = paths.iter().map(|p| ff(p, &[])).collect();
        build("Audiobooks", &owned)
    }

    fn names(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn needs_ebook_is_audio_and_missing() {
        let cases = [
            (true, true, true),    // gap
            (true, false, false),  // covered audiobook
            (false, false, false), // covered container
            (false, true, false),  // plain container
        ];
        for (audio, missing, want) in cases {
            let node = Node {
                name: "X".to_string(),
                rel_path: "X".to_string(),
                directly_holds_audio: audio,
                missing_ebook: missing,
                children: Vec::new(),
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            };
            assert_eq!(node.needs_ebook(), want, "audio={audio} missing={missing}");
        }
    }

    #[test]
    fn siblings_use_natural_order() {
        let roots = forest(&["Series/Book 2", "Series/Book 10"]);
        assert_eq!(names(&roots), vec!["Series"]);
        assert_eq!(names(&roots[0].children), vec!["Book 2", "Book 10"]);
    }

    #[test]
    fn order_is_case_insensitive() {
        let roots = forest(&["X/banana", "X/Apple"]);
        assert_eq!(names(&roots[0].children), vec!["Apple", "banana"]);
    }

    #[test]
    fn nesting_mirrors_the_path_with_no_stray_nodes() {
        let roots = forest(&["A/B/C"]);
        assert_eq!(names(&roots), vec!["A"]);
        assert!(!roots[0].needs_ebook());
        assert_eq!(names(&roots[0].children), vec!["B"]);
        assert!(!roots[0].children[0].needs_ebook());
        let c = &roots[0].children[0].children[0];
        assert_eq!(c.name, "C");
        assert_eq!(c.rel_path, "A/B/C");
        assert!(c.needs_ebook());
        assert!(c.children.is_empty());
    }

    #[test]
    fn top_level_nodes_form_a_sorted_forest() {
        let roots = forest(&["B/y", "A/x"]);
        assert_eq!(names(&roots), vec!["A", "B"]);
    }

    #[test]
    fn loose_audio_in_the_root_becomes_a_flagged_root_node() {
        // The scanner reports the root itself as the empty relative path
        // (see ADR-0005). It must surface as a flagged node pinned first.
        let owned = vec![ff("", &[]), ff("Andy Weir/Artemis", &[])];
        let roots = build("Audiobooks", &owned);
        assert_eq!(names(&roots), vec!["Audiobooks", "Andy Weir"]);
        assert!(roots[0].needs_ebook());
        assert_eq!(roots[0].rel_path, ".");
        assert!(roots[0].children.is_empty());
    }

    #[test]
    fn build_carries_audio_files_onto_a_flagged_leaf() {
        let forest = build("Audiobooks", &[ff("Author/Book", &["01.mp3", "02.mp3"])]);
        assert_eq!(
            find(&forest, "Author/Book").unwrap().audio_files,
            vec!["01.mp3".to_string(), "02.mp3".to_string()]
        );
        // The inferred container above the leaf holds no audio of its own.
        assert!(find(&forest, "Author").unwrap().audio_files.is_empty());
    }

    fn gap_leaf(name: &str, rel: &str) -> Node {
        Node {
            name: name.to_string(),
            rel_path: rel.to_string(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: Vec::new(),
            cover_files: Vec::new(),
            audio_files: Vec::new(),
        }
    }

    fn sf(rel: &str, audio: bool, missing: bool) -> ScannedFolder {
        ScannedFolder {
            rel_path: PathBuf::from(rel),
            directly_holds_audio: audio,
            missing_ebook: missing,
            cover_files: Vec::new(),
            audio_files: Vec::new(),
        }
    }

    fn sf_audio(rel: &str, audio_files: &[&str]) -> ScannedFolder {
        ScannedFolder {
            rel_path: PathBuf::from(rel),
            directly_holds_audio: true,
            missing_ebook: true,
            cover_files: Vec::new(),
            audio_files: audio_files.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Find a node by its `/`-joined relative path, descending the forest.
    fn find<'a>(forest: &'a [Node], rel: &str) -> Option<&'a Node> {
        for node in forest {
            if node.rel_path == rel {
                return Some(node);
            }
            if let Some(found) = find(&node.children, rel) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn build_all_carries_all_four_kinds_sorted() {
        let folders = vec![
            sf("", false, true),               // the root, no loose audio: dropped
            sf("Series", false, false),        // covered container
            sf("Series/Book 10", true, false), // covered audiobook
            sf("Series/Book 2", true, false),  // covered audiobook
            sf("Gap Author", false, true),     // plain container
            sf("Gap Author/Book", true, true), // gap
        ];
        let forest = build("Audiobooks", &folders);

        // Top level is natural-sorted and the root has no `.` node.
        assert_eq!(names(&forest), vec!["Gap Author", "Series"]);

        let series = find(&forest, "Series").unwrap();
        assert_eq!(
            (series.directly_holds_audio, series.missing_ebook),
            (false, false)
        );
        // Children natural-sorted: Book 2 before Book 10.
        assert_eq!(names(&series.children), vec!["Book 2", "Book 10"]);

        let gap = find(&forest, "Gap Author/Book").unwrap();
        assert!(gap.needs_ebook());
        let covered_book = find(&forest, "Series/Book 2").unwrap();
        assert!(!covered_book.missing_ebook);
        assert!(covered_book.directly_holds_audio);
    }

    #[test]
    fn build_all_pins_a_loose_audio_root_as_the_dot_node() {
        let folders = vec![
            sf("", true, true), // root holds loose uncovered audio
            sf("Andy Weir", false, true),
            sf("Andy Weir/Artemis", true, true),
        ];
        let forest = build("Audiobooks", &folders);
        assert_eq!(names(&forest), vec!["Audiobooks", "Andy Weir"]);
        assert_eq!(forest[0].rel_path, ".");
        assert!(forest[0].needs_ebook());
        assert!(forest[0].children.is_empty());
    }

    fn sf_cov(rel: &str, audio: bool, missing: bool, cover_files: &[&str]) -> ScannedFolder {
        ScannedFolder {
            rel_path: PathBuf::from(rel),
            directly_holds_audio: audio,
            missing_ebook: missing,
            cover_files: cover_files.iter().map(|s| s.to_string()).collect(),
            audio_files: Vec::new(),
        }
    }

    #[test]
    fn build_all_carries_cover_files_onto_the_node() {
        let folders = vec![
            sf_cov("Series", false, false, &["Series.epub"]),
            sf_cov("Series/Book 1", true, false, &[]),
        ];
        let forest = build("Audiobooks", &folders);
        assert_eq!(
            find(&forest, "Series").unwrap().cover_files,
            vec!["Series.epub".to_string()]
        );
        assert!(
            find(&forest, "Series/Book 1")
                .unwrap()
                .cover_files
                .is_empty()
        );
    }

    #[test]
    fn build_all_carries_audio_files_onto_the_node() {
        let folders = vec![sf_audio("Book", &["01 - One.mp3", "02 - Two.mp3"])];
        let forest = build("Audiobooks", &folders);
        assert_eq!(
            find(&forest, "Book").unwrap().audio_files,
            vec!["01 - One.mp3".to_string(), "02 - Two.mp3".to_string()]
        );
    }

    #[test]
    fn build_all_carries_root_cover_files_onto_the_dot_node() {
        let folders = vec![
            sf_cov("", true, false, &[".no_ebook"]),
            sf_cov("Author", false, true, &[]),
        ];
        let forest = build("Audiobooks", &folders);
        assert_eq!(forest[0].rel_path, ".");
        assert_eq!(forest[0].cover_files, vec![".no_ebook".to_string()]);
    }

    /// The unified `build` (taking ScannedFolder) must emit a forest equivalent
    /// to today's `build_all` for a show-all input: every input folder yields a
    /// node, the `.` root node only appears when the empty-path entry holds
    /// audio, and siblings come out natural-sorted. The expected forest is
    /// hand-constructed so the test survives `build_all`'s deletion in Step 3.
    #[test]
    fn unified_build_emits_a_show_all_forest_for_every_input_folder() {
        let folders = vec![
            sf("", false, true),
            sf("AuthorA", false, true),
            sf("AuthorA/Book", true, true),
            sf_cov("Series", false, false, &["Series.epub"]),
            sf("Series/Book 1", true, false),
            sf("Series/Book 10", true, false),
            sf("Series/Book 2", true, false),
        ];
        let got = build("Audiobooks", &folders);
        let expected = vec![
            Node {
                name: "AuthorA".to_string(),
                rel_path: "AuthorA".to_string(),
                directly_holds_audio: false,
                missing_ebook: true,
                children: vec![Node {
                    name: "Book".to_string(),
                    rel_path: "AuthorA/Book".to_string(),
                    directly_holds_audio: true,
                    missing_ebook: true,
                    children: Vec::new(),
                    cover_files: Vec::new(),
                    audio_files: Vec::new(),
                }],
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            },
            Node {
                name: "Series".to_string(),
                rel_path: "Series".to_string(),
                directly_holds_audio: false,
                missing_ebook: false,
                children: vec![
                    Node {
                        name: "Book 1".to_string(),
                        rel_path: "Series/Book 1".to_string(),
                        directly_holds_audio: true,
                        missing_ebook: false,
                        children: Vec::new(),
                        cover_files: Vec::new(),
                        audio_files: Vec::new(),
                    },
                    Node {
                        name: "Book 2".to_string(),
                        rel_path: "Series/Book 2".to_string(),
                        directly_holds_audio: true,
                        missing_ebook: false,
                        children: Vec::new(),
                        cover_files: Vec::new(),
                        audio_files: Vec::new(),
                    },
                    Node {
                        name: "Book 10".to_string(),
                        rel_path: "Series/Book 10".to_string(),
                        directly_holds_audio: true,
                        missing_ebook: false,
                        children: Vec::new(),
                        cover_files: Vec::new(),
                        audio_files: Vec::new(),
                    },
                ],
                cover_files: vec!["Series.epub".to_string()],
                audio_files: Vec::new(),
            },
        ];
        assert_eq!(got, expected);
    }

    /// The unified `build` against a gaps-filtered input must match today's
    /// `build` semantics: only flagged subtrees survive, intermediate containers
    /// above kept gaps are inferred with `directly_holds_audio = false,
    /// missing_ebook = true`, and order is the same natural-sorted forest.
    #[test]
    fn unified_build_matches_today_build_for_a_gaps_input() {
        let folders = [
            sf("", false, true),
            sf("AuthorA", false, true),
            sf_audio("AuthorA/Book 2", &["01.mp3"]),
            sf_audio("AuthorA/Book 10", &["01.mp3"]),
            sf_cov("Series", false, false, &["Series.epub"]),
            sf("Series/Book 1", true, false), // covered, dropped by the filter
        ];
        let flagged_input: Vec<ScannedFolder> = folders
            .iter()
            .filter(|f| f.directly_holds_audio && f.missing_ebook)
            .cloned()
            .collect();
        let unified = build("Audiobooks", &flagged_input);
        let expected = vec![Node {
            name: "AuthorA".to_string(),
            rel_path: "AuthorA".to_string(),
            directly_holds_audio: false,
            missing_ebook: true,
            children: vec![
                Node {
                    name: "Book 2".to_string(),
                    rel_path: "AuthorA/Book 2".to_string(),
                    directly_holds_audio: true,
                    missing_ebook: true,
                    children: Vec::new(),
                    cover_files: Vec::new(),
                    audio_files: vec!["01.mp3".to_string()],
                },
                Node {
                    name: "Book 10".to_string(),
                    rel_path: "AuthorA/Book 10".to_string(),
                    directly_holds_audio: true,
                    missing_ebook: true,
                    children: Vec::new(),
                    cover_files: Vec::new(),
                    audio_files: vec!["01.mp3".to_string()],
                },
            ],
            cover_files: Vec::new(),
            audio_files: Vec::new(),
        }];
        assert_eq!(unified, expected);
    }

    #[test]
    fn has_gap_within_sees_a_gap_at_or_below_a_node() {
        let container = Node {
            name: "A".to_string(),
            rel_path: "A".to_string(),
            directly_holds_audio: false,
            missing_ebook: true,
            children: vec![gap_leaf("B", "A/B")],
            cover_files: Vec::new(),
            audio_files: Vec::new(),
        };
        assert!(container.has_gap_within(), "a descendant gap counts");

        let covered = Node {
            name: "Series".to_string(),
            rel_path: "Series".to_string(),
            directly_holds_audio: false,
            missing_ebook: false,
            children: vec![Node {
                name: "Book".to_string(),
                rel_path: "Series/Book".to_string(),
                directly_holds_audio: true,
                missing_ebook: false,
                children: Vec::new(),
                cover_files: Vec::new(),
                audio_files: Vec::new(),
            }],
            cover_files: Vec::new(),
            audio_files: Vec::new(),
        };
        assert!(!covered.has_gap_within(), "a fully covered branch has none");
    }
}
