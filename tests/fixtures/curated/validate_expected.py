#!/usr/bin/env python3
"""Check that expected.json stays consistent with the curated fixture tree.

There is no scanner yet, so expected.json is hand-authored. This script does not
decide verdicts; it confirms the contract and the tree agree:

  - every path listed in expected.json exists as a directory under the root
  - every folder that directly holds a real audio file is accounted for:
    listed in flagged or covered, or inside an excluded subtree
  - flagged, covered, and excluded are pairwise disjoint, and no absent path is
    also reported
  - the containers list equals exactly the proper ancestors of the flagged folders
  - no absent folder has a flagged folder beneath it

Run from anywhere:  python3 tests/fixtures/curated/validate_expected.py
Prints every problem and exits non-zero if anything is inconsistent.
"""
import json
import os
import sys

AUDIO_EXTS = {".mp3", ".m4a", ".m4b"}
HERE = os.path.dirname(os.path.abspath(__file__))
EXPECTED = os.path.join(HERE, "expected.json")


def paths_of(entries):
    return [e["path"] for e in entries]


def is_audio(name):
    if name.startswith("."):  # skips AppleDouble ._ and hidden dotfiles (.beets, .gitkeep)
        return False
    return os.path.splitext(name)[1].lower() in AUDIO_EXTS


def audio_folders(root):
    """Relative folders that directly contain at least one real audio file."""
    found = set()
    for dirpath, _dirnames, filenames in os.walk(root):
        if any(is_audio(n) for n in filenames):
            rel = os.path.relpath(dirpath, root)
            if rel != ".":
                found.add(rel)
    return found


def ancestors(rel):
    parts = rel.split("/")
    return ["/".join(parts[:i]) for i in range(1, len(parts))]


def main():
    with open(EXPECTED, encoding="utf-8") as fh:
        data = json.load(fh)
    root = os.path.join(HERE, data["library_root"])
    if not os.path.isdir(root):
        print(f"library_root not found: {root}")
        return 1

    flagged = set(paths_of(data.get("flagged", [])))
    covered = set(paths_of(data.get("covered", [])))
    excluded = set(paths_of(data.get("excluded", [])))
    containers = set(paths_of(data.get("containers", [])))
    absent = set(paths_of(data.get("absent", [])))

    problems = []

    # 1. every listed path exists as a directory
    for label, group in (("flagged", flagged), ("covered", covered),
                         ("excluded", excluded), ("containers", containers),
                         ("absent", absent)):
        for p in sorted(group):
            if not os.path.isdir(os.path.join(root, p)):
                problems.append(f"{label} path is not a directory in the tree: {p}")

    # 2. every audio folder is accounted for
    def under_excluded(rel):
        return any(rel == e or rel.startswith(e + "/") for e in excluded)

    for rel in sorted(audio_folders(root)):
        if rel in flagged or rel in covered or under_excluded(rel):
            continue
        problems.append(f"audio folder is not classified (flagged/covered/excluded): {rel}")

    # 3. disjointness
    if flagged & covered:
        problems.append(f"paths in both flagged and covered: {sorted(flagged & covered)}")
    if flagged & excluded:
        problems.append(f"paths in both flagged and excluded: {sorted(flagged & excluded)}")
    if covered & excluded:
        problems.append(f"paths in both covered and excluded: {sorted(covered & excluded)}")
    reported = flagged | covered | excluded | containers
    if absent & reported:
        problems.append(f"absent paths that are also reported: {sorted(absent & reported)}")

    # 4. containers == proper ancestors of flagged
    expected_containers = set()
    for rel in flagged:
        expected_containers.update(ancestors(rel))
    missing = sorted(expected_containers - containers)
    extra = sorted(containers - expected_containers)
    if missing:
        problems.append(f"containers missing (ancestors of flagged): {missing}")
    if extra:
        problems.append(f"containers not an ancestor of any flagged folder: {extra}")

    # 5. no absent folder has a flagged descendant
    for p in sorted(absent):
        kids = sorted(f for f in flagged if f.startswith(p + "/"))
        if kids:
            problems.append(f"absent folder has flagged descendants (should be a container): {p} -> {kids}")

    if problems:
        print(f"FAIL: {len(problems)} consistency problem(s):")
        for pr in problems:
            print("  -", pr)
        return 1
    print("OK: expected.json is consistent with the curated fixture tree.")
    print(f"  flagged={len(flagged)} covered={len(covered)} excluded={len(excluded)} "
          f"containers={len(containers)} absent={len(absent)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
