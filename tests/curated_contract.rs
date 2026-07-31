//! Integration test: the scanner's flagged set and the tree's container set
//! must match `tests/fixtures/curated/expected.json`, the contract from the
//! design. `expected.json` is the single source of truth this test reads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use missing_ebooks::config::Config;
use missing_ebooks::scanner::{
    DirIndex, RootScan, ScanInputs, ScanSettings, ScannedFolder, scan_warm,
};
use missing_ebooks::tree::{Node, RootState, ViewMode, build};

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
    #[serde(default)]
    cover_files: Vec<String>,
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
    let folders = scan_warm(&root, &expected_settings(&expected), &DirIndex::new()).0;
    let got: BTreeSet<String> = folders
        .iter()
        .filter(|f| f.directly_holds_audio && f.missing_ebook)
        .map(|f| f.rel_path.to_string_lossy().replace('\\', "/"))
        .collect();
    let want: BTreeSet<String> = expected.flagged.iter().map(|f| f.path.clone()).collect();
    assert_eq!(got, want);
}

#[test]
fn tree_container_set_matches_the_contract() {
    let expected = load_expected();
    let flagged_paths: Vec<ScannedFolder> = expected
        .flagged
        .iter()
        .map(|f| ScannedFolder {
            rel_path: PathBuf::from(&f.path),
            directly_holds_audio: true,
            missing_ebook: true,
            cover_files: std::sync::Arc::from(Vec::<String>::new()),
            audio_files: std::sync::Arc::from(Vec::<String>::new()),
        })
        .collect();
    let scan = RootScan::Walked {
        canonical_path: PathBuf::from("/Audiobooks"),
        folders: flagged_paths,
        skipped_dirs: 0,
        depth_capped_dirs: 0,
    };
    let forest = match build(&scan, ViewMode::All) {
        RootState::Forest(f) => f,
        other => panic!("expected Forest, got {other:?}"),
    };

    let mut got_flagged = BTreeSet::new();
    let mut got_containers = BTreeSet::new();
    collect(&forest, &mut got_flagged, &mut got_containers);

    let want_flagged: BTreeSet<String> = expected.flagged.iter().map(|f| f.path.clone()).collect();
    let want_containers: BTreeSet<String> =
        expected.containers.iter().map(|f| f.path.clone()).collect();
    assert_eq!(got_flagged, want_flagged);
    assert_eq!(got_containers, want_containers);
}

fn collect_all(nodes: &[Node], out: &mut BTreeMap<String, (bool, bool, Vec<String>)>) {
    for node in nodes {
        out.insert(
            node.rel_path.clone(),
            (
                node.directly_holds_audio,
                node.missing_ebook,
                node.cover_files.clone(),
            ),
        );
        collect_all(&node.children, out);
    }
}

#[test]
fn scan_and_build_match_the_contract() {
    let expected = load_expected();
    let root = fixture_dir().join("Audiobooks");
    let folders = scan_warm(&root, &expected_settings(&expected), &DirIndex::new()).0;
    let scan = RootScan::Walked {
        canonical_path: PathBuf::from("/Audiobooks"),
        folders,
        skipped_dirs: 0,
        depth_capped_dirs: 0,
    };
    let forest = match build(&scan, ViewMode::All) {
        RootState::Forest(f) => f,
        other => panic!("expected Forest, got {other:?}"),
    };

    let mut got: BTreeMap<String, (bool, bool, Vec<String>)> = BTreeMap::new();
    collect_all(&forest, &mut got);

    let want: BTreeMap<String, (bool, bool, Vec<String>)> = expected
        .all
        .iter()
        .map(|f| {
            (
                f.path.clone(),
                (
                    f.directly_holds_audio,
                    f.missing_ebook,
                    f.cover_files.clone(),
                ),
            )
        })
        .collect();
    assert_eq!(got, want);
}
