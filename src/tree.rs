//! Build the flagged-only forest for one root from the flat list of flagged
//! folder paths (relative to the root). Internal nodes are exactly the
//! containers (ancestors of a flagged folder); because only flagged paths are
//! inserted, no empty branch can appear. Siblings are ordered by case-
//! insensitive natural sort, so `Book 2` precedes `Book 10`. The empty relative
//! path is the library root itself (loose root audio, see ADR-0005): it becomes
//! one flagged node named after the root with relative path `.`, pinned ahead of
//! the forest. The types derive `Serialize` so a future JSON API can return them
//! unchanged.

use std::path::{Component, PathBuf};

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
    /// `scanner::ScannedFolder::cover_files`; empty in gaps-only.
    pub cover_files: Vec<String>,
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

/// Build the forest of top-level nodes for one root. `root_name` names the node
/// emitted when the root itself is flagged by loose root audio (see ADR-0005).
///
/// Each path in `flagged` is expected to be root-relative, as the scanner
/// produces it. Non-normal components (a leading `/` or `..`) are dropped, so an
/// absolute path would lose its prefix.
#[must_use]
pub fn build(root_name: &str, flagged: &[PathBuf]) -> Vec<Node> {
    let mut roots: Vec<Node> = Vec::new();
    let mut root_flagged = false;
    for path in flagged {
        let components: Vec<String> = path
            .components()
            .filter_map(|c| match c {
                Component::Normal(os) => Some(os.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if components.is_empty() {
            // The empty relative path is the library root itself: it directly
            // holds uncovered audio, so the root is a flagged gap (ADR-0005).
            root_flagged = true;
            continue;
        }
        insert(&mut roots, &components, "");
    }
    sort_forest(&mut roots);
    if root_flagged {
        // Pin the flagged root ahead of its natural-sorted children. Relative
        // path "." is the root itself; rendering substitutes the root label.
        roots.insert(
            0,
            Node {
                name: root_name.to_string(),
                rel_path: ".".to_string(),
                // Loose root audio: a gap. The old `flagged: true`.
                directly_holds_audio: true,
                missing_ebook: true,
                children: Vec::new(),
                cover_files: Vec::new(),
            },
        );
    }
    roots
}

/// Build the full-tree forest for one root from `scanner::scan_all` output. Every
/// folder carries its own two facts. Unlike `build`, intermediate nodes are not
/// inferred as bare containers: `scan_all` emits every folder, so each node's own
/// entry sets its facts. `root_name` names the `.` node emitted when the root
/// itself directly holds audio (see ADR-0005).
#[must_use]
pub fn build_all(root_name: &str, folders: &[crate::scanner::ScannedFolder]) -> Vec<Node> {
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
            // The empty relative path is the library root itself.
            root_entry = Some(folder);
            continue;
        }
        insert_all(&mut roots, &components, "", folder);
    }
    sort_forest(&mut roots);
    if let Some(entry) = root_entry
        && entry.directly_holds_audio
    {
        // The root directly holds audio: surface it as a node, pinned ahead of the
        // author forest (see ADR-0005). In show-all it shows even when covered.
        roots.insert(
            0,
            Node {
                name: root_name.to_string(),
                rel_path: ".".to_string(),
                directly_holds_audio: true,
                missing_ebook: entry.missing_ebook,
                children: Vec::new(),
                cover_files: entry.cover_files.clone(),
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
                // processed. `scan_all` emits every folder, so that always happens.
                // A plain container is the safe neutral if it somehow were not.
                directly_holds_audio: false,
                missing_ebook: true,
                children: Vec::new(),
                cover_files: Vec::new(),
            });
            siblings.len() - 1
        }
    };
    if tail.is_empty() {
        siblings[idx].directly_holds_audio = folder.directly_holds_audio;
        siblings[idx].missing_ebook = folder.missing_ebook;
        siblings[idx].cover_files = folder.cover_files.clone();
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

fn insert(siblings: &mut Vec<Node>, components: &[String], parent_rel: &str) {
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
                // A container above a gap: uncovered (the pruned scan never emits
                // a covered node), holds no direct audio yet. The old default.
                directly_holds_audio: false,
                missing_ebook: true,
                children: Vec::new(),
                cover_files: Vec::new(),
            });
            siblings.len() - 1
        }
    };
    if tail.is_empty() {
        // This path's tail ends here: the folder directly holds audio. With
        // `missing_ebook` already true, this is a gap. The old `flagged = true`.
        siblings[idx].directly_holds_audio = true;
    } else {
        insert(&mut siblings[idx].children, tail, &rel_path);
    }
}

/// Remove the node addressed by `rel` (a `/`-joined root-relative path) from the
/// forest, then prune any ancestor left as an empty, non-flagged container. A
/// path that is already gone, because a rescan landed first or a button was
/// double-clicked, is a silent no-op. The `.` root sentinel is handled by the
/// caller (see `service::apply_mark`), not here.
pub fn remove_subtree(forest: &mut Vec<Node>, rel: &str) {
    let components: Vec<&str> = rel.split('/').collect();
    remove_at(forest, &components, "");
}

fn remove_at(siblings: &mut Vec<Node>, components: &[&str], parent_rel: &str) {
    let Some((head, tail)) = components.split_first() else {
        return;
    };
    let cur_rel = child_rel(parent_rel, head);
    let Some(idx) = siblings.iter().position(|n| n.rel_path == cur_rel) else {
        return;
    };
    if tail.is_empty() {
        siblings.remove(idx);
    } else {
        remove_at(&mut siblings[idx].children, tail, &cur_rel);
        if siblings[idx].children.is_empty() && !siblings[idx].needs_ebook() {
            siblings.remove(idx);
        }
    }
}

/// Cover the node addressed by `rel` (a `/`-joined root-relative path) and its
/// whole subtree: flip `missing_ebook` to false on it and every descendant,
/// leaving the nodes in the forest. The show-all counterpart to `remove_subtree`:
/// a marked folder turns from gap to covered rather than disappearing. An absent
/// path is a silent no-op. The `.` root sentinel is handled by the caller via
/// `cover_all`.
pub fn cover_subtree(forest: &mut [Node], rel: &str) {
    let components: Vec<&str> = rel.split('/').collect();
    cover_at(forest, &components, "");
}

fn cover_at(siblings: &mut [Node], components: &[&str], parent_rel: &str) {
    let Some((head, tail)) = components.split_first() else {
        return;
    };
    let cur_rel = child_rel(parent_rel, head);
    let Some(node) = siblings.iter_mut().find(|n| n.rel_path == cur_rel) else {
        return;
    };
    if tail.is_empty() {
        cover_node(node);
    } else {
        cover_at(&mut node.children, tail, &cur_rel);
    }
}

/// Cover every node in the forest. Used when the library root itself is marked
/// (`rel == "."`): coverage from the root flows to everything beneath it.
pub fn cover_all(forest: &mut [Node]) {
    for node in forest {
        cover_node(node);
    }
}

fn cover_node(node: &mut Node) {
    node.missing_ebook = false;
    for child in &mut node.children {
        cover_node(child);
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

    fn forest(paths: &[&str]) -> Vec<Node> {
        let owned: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
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
        // (see ADR-0005); it must surface as a flagged node pinned first.
        let owned = vec![PathBuf::from(""), PathBuf::from("Andy Weir/Artemis")];
        let roots = build("Audiobooks", &owned);
        assert_eq!(names(&roots), vec!["Audiobooks", "Andy Weir"]);
        assert!(roots[0].needs_ebook());
        assert_eq!(roots[0].rel_path, ".");
        assert!(roots[0].children.is_empty());
    }

    fn gap_leaf(name: &str, rel: &str) -> Node {
        Node {
            name: name.to_string(),
            rel_path: rel.to_string(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: Vec::new(),
            cover_files: Vec::new(),
        }
    }

    #[test]
    fn remove_subtree_removes_an_addressed_leaf_and_prunes_the_container() {
        let mut forest = vec![Node {
            name: "Author".to_string(),
            rel_path: "Author".to_string(),
            directly_holds_audio: false,
            missing_ebook: true,
            children: vec![gap_leaf("Book", "Author/Book")],
            cover_files: Vec::new(),
        }];
        remove_subtree(&mut forest, "Author/Book");
        assert!(
            forest.is_empty(),
            "removing the only child prunes the container"
        );
    }

    #[test]
    fn remove_subtree_on_a_container_removes_the_whole_subtree() {
        let mut forest = vec![Node {
            name: "Author".to_string(),
            rel_path: "Author".to_string(),
            directly_holds_audio: false,
            missing_ebook: true,
            children: vec![
                gap_leaf("Book 1", "Author/Book 1"),
                gap_leaf("Book 2", "Author/Book 2"),
            ],
            cover_files: Vec::new(),
        }];
        remove_subtree(&mut forest, "Author");
        assert!(forest.is_empty());
    }

    #[test]
    fn remove_subtree_keeps_a_flagged_node_when_its_child_goes() {
        let mut forest = vec![Node {
            name: "Author".to_string(),
            rel_path: "Author".to_string(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: vec![gap_leaf("Book", "Author/Book")],
            cover_files: Vec::new(),
        }];
        remove_subtree(&mut forest, "Author/Book");
        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].name, "Author");
        assert!(forest[0].children.is_empty());
        assert!(forest[0].needs_ebook());
    }

    #[test]
    fn remove_subtree_on_an_absent_path_is_a_noop() {
        let mut forest = vec![gap_leaf("Author", "Author")];
        remove_subtree(&mut forest, "Ghost");
        assert_eq!(forest.len(), 1);
    }

    fn sf(rel: &str, audio: bool, missing: bool) -> ScannedFolder {
        ScannedFolder {
            rel_path: PathBuf::from(rel),
            directly_holds_audio: audio,
            missing_ebook: missing,
            cover_files: Vec::new(),
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
        let forest = build_all("Audiobooks", &folders);

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
        let forest = build_all("Audiobooks", &folders);
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
        }
    }

    #[test]
    fn build_all_carries_cover_files_onto_the_node() {
        let folders = vec![
            sf_cov("Series", false, false, &["Series.epub"]),
            sf_cov("Series/Book 1", true, false, &[]),
        ];
        let forest = build_all("Audiobooks", &folders);
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
    fn build_all_carries_root_cover_files_onto_the_dot_node() {
        let folders = vec![
            sf_cov("", true, false, &[".no_ebook"]),
            sf_cov("Author", false, true, &[]),
        ];
        let forest = build_all("Audiobooks", &folders);
        assert_eq!(forest[0].rel_path, ".");
        assert_eq!(forest[0].cover_files, vec![".no_ebook".to_string()]);
    }

    #[test]
    fn cover_subtree_covers_the_addressed_node_and_descendants() {
        let mut forest = vec![Node {
            name: "Series".to_string(),
            rel_path: "Series".to_string(),
            directly_holds_audio: false,
            missing_ebook: true,
            children: vec![
                gap_leaf("Book 1", "Series/Book 1"),
                gap_leaf("Book 2", "Series/Book 2"),
            ],
            cover_files: Vec::new(),
        }];
        cover_subtree(&mut forest, "Series");
        let series = find(&forest, "Series").unwrap();
        assert!(!series.missing_ebook);
        for child in &series.children {
            assert!(!child.missing_ebook, "descendant flips to covered");
            assert!(child.directly_holds_audio, "audio fact is untouched");
        }
    }

    #[test]
    fn cover_subtree_leaves_siblings_untouched() {
        let mut forest = vec![Node {
            name: "Author".to_string(),
            rel_path: "Author".to_string(),
            directly_holds_audio: false,
            missing_ebook: true,
            children: vec![
                gap_leaf("Marked", "Author/Marked"),
                gap_leaf("Other", "Author/Other"),
            ],
            cover_files: Vec::new(),
        }];
        cover_subtree(&mut forest, "Author/Marked");
        assert!(!find(&forest, "Author/Marked").unwrap().missing_ebook);
        assert!(
            find(&forest, "Author/Other").unwrap().missing_ebook,
            "sibling untouched"
        );
    }

    #[test]
    fn cover_subtree_on_an_absent_path_is_a_noop() {
        let mut forest = vec![gap_leaf("Author", "Author")];
        cover_subtree(&mut forest, "Ghost");
        assert!(forest[0].missing_ebook);
    }

    #[test]
    fn cover_all_flips_every_node() {
        let mut forest = vec![
            Node {
                name: "A".to_string(),
                rel_path: "A".to_string(),
                directly_holds_audio: false,
                missing_ebook: true,
                children: vec![gap_leaf("B", "A/B")],
                cover_files: Vec::new(),
            },
            gap_leaf("C", "C"),
        ];
        cover_all(&mut forest);
        assert!(!find(&forest, "A").unwrap().missing_ebook);
        assert!(!find(&forest, "A/B").unwrap().missing_ebook);
        assert!(!find(&forest, "C").unwrap().missing_ebook);
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
            }],
            cover_files: Vec::new(),
        };
        assert!(!covered.has_gap_within(), "a fully covered branch has none");
    }
}
