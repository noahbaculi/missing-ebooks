//! Integration test: the scanner's flagged set and the tree's container set
//! must match tests/fixtures/curated/expected.json, the contract from the
//! design. expected.json is the single source of truth; this test reads it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use missing_ebooks::config::Config;
use missing_ebooks::scanner::{ScanInputs, ScanSettings, scan, scan_all};
use missing_ebooks::tree::{Node, build, build_all};

use serde::Deserialize;

#[derive(Deserialize)]
struct Expected {
    config: ExpectedConfig,
    flagged: Vec<Finding>,
    containers: Vec<Finding>,
    all: Vec<AllFolder>,
}

#[derive(Deserialize)]
struct AllFolder {
    path: String,
    directly_holds_audio: bool,
    missing_ebook: bool,
}

#[derive(Deserialize)]
struct ExpectedConfig {
    exclude_globs: Vec<String>,
    excluded_dirs: Vec<String>,
}

#[derive(Deserialize)]
struct Finding {
    path: String,
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/curated")
}

fn load_expected() -> Expected {
    let text = std::fs::read_to_string(fixture_dir().join("expected.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn expected_settings(expected: &Expected) -> ScanSettings {
    let defaults = Config::default();
    ScanSettings::compile(ScanInputs {
        audio_exts: &defaults.audio_exts,
        ebook_exts: &defaults.ebook_exts,
        excluded_dirs: &expected.config.excluded_dirs,
        exclude_globs: &expected.config.exclude_globs,
    })
    .unwrap()
}

fn collect(nodes: &[Node], flagged: &mut BTreeSet<String>, containers: &mut BTreeSet<String>) {
    for node in nodes {
        if node.needs_ebook() {
            flagged.insert(node.rel_path.clone());
        } else {
            containers.insert(node.rel_path.clone());
        }
        collect(&node.children, flagged, containers);
    }
}

#[test]
fn scanner_flagged_set_matches_the_contract() {
    let expected = load_expected();
    let root = fixture_dir().join("Audiobooks");
    let got: BTreeSet<String> = scan(&root, &expected_settings(&expected))
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    let want: BTreeSet<String> = expected.flagged.iter().map(|f| f.path.clone()).collect();
    assert_eq!(got, want);
}

#[test]
fn tree_container_set_matches_the_contract() {
    let expected = load_expected();
    let flagged_paths: Vec<PathBuf> = expected
        .flagged
        .iter()
        .map(|f| PathBuf::from(&f.path))
        .collect();
    let forest = build("Audiobooks", &flagged_paths);

    let mut got_flagged = BTreeSet::new();
    let mut got_containers = BTreeSet::new();
    collect(&forest, &mut got_flagged, &mut got_containers);

    let want_flagged: BTreeSet<String> = expected.flagged.iter().map(|f| f.path.clone()).collect();
    let want_containers: BTreeSet<String> =
        expected.containers.iter().map(|f| f.path.clone()).collect();
    assert_eq!(got_flagged, want_flagged);
    assert_eq!(got_containers, want_containers);
}

fn collect_all(nodes: &[Node], out: &mut BTreeMap<String, (bool, bool)>) {
    for node in nodes {
        out.insert(
            node.rel_path.clone(),
            (node.directly_holds_audio, node.missing_ebook),
        );
        collect_all(&node.children, out);
    }
}

#[test]
fn scan_all_and_build_all_match_the_contract() {
    let expected = load_expected();
    let root = fixture_dir().join("Audiobooks");
    let folders = scan_all(&root, &expected_settings(&expected));
    let forest = build_all("Audiobooks", &folders);

    let mut got: BTreeMap<String, (bool, bool)> = BTreeMap::new();
    collect_all(&forest, &mut got);

    let want: BTreeMap<String, (bool, bool)> = expected
        .all
        .iter()
        .map(|f| (f.path.clone(), (f.directly_holds_audio, f.missing_ebook)))
        .collect();
    assert_eq!(got, want);
}
