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
}

impl Node {
    /// A gap: this folder holds audio and is missing an ebook. Reproduces the old
    /// `flagged` value. CONTEXT.md's "flagged folder".
    #[must_use]
    pub fn needs_ebook(&self) -> bool {
        self.directly_holds_audio && self.missing_ebook
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
            });
            siblings.len() - 1
        }
    };
    if tail.is_empty() {
        siblings[idx].directly_holds_audio = folder.directly_holds_audio;
        siblings[idx].missing_ebook = folder.missing_ebook;
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
}
