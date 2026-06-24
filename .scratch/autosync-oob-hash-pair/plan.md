# Autosync `(oob_html, hash)` Pair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Concentrate the `render_oob_section`/`stable_hash` lockstep in `src/autosync.rs` into one helper shared by `snapshot_and_seed` and `compute_pushes`, so the ADR-0024 byte-equality invariant is structural rather than authorship-enforced.

**Architecture:** Add one private helper `rendered_oob_with_hash` to `src/autosync.rs`. Route both autosync paths through it. No public API change, no ADR change, no behavior change.

**Tech Stack:** Rust, `cargo test`, existing `mise` pre-commit hook.

## Global Constraints

- All changes confined to `src/autosync.rs`. No other file is touched.
- No public API surface changes. The new helper is module-private.
- No behavior change. Every existing autosync test must continue to pass unmodified.
- Pre-commit hook runs fmt, clippy, and `cargo doc -D warnings`. Never bypass with `--no-verify`.
- Commits follow Conventional Commits. `refactor(autosync): ...` for the call-site routings; `test(autosync): ...` for the new test if its commit stands alone.

---

### Task 1: Add `rendered_oob_with_hash` helper with its pair-contract test

**Files:**
- Modify: `src/autosync.rs` (add helper near `render_oob_section` at L70, add test in the existing `#[cfg(test)] mod tests` block at end of file)

**Interfaces:**
- Produces: `fn rendered_oob_with_hash(scan: &RootScan, root_idx: usize, mode: ViewMode, links: &[SearchLink]) -> (String, u64)`. Module-private. Tasks 2 and 3 will consume it.

- [ ] **Step 1: Write the failing test**

Add this test inside the existing `#[cfg(test)] mod tests { ... }` block in `src/autosync.rs`, alongside `render_oob_section_html_carries_total_audiobooks_for_a_walked_root`:

```rust
#[test]
fn rendered_oob_with_hash_returns_render_oob_section_paired_with_its_stable_hash() {
    use crate::scanner::ScannedFolder;
    use std::path::PathBuf;

    // Mirror the walked-RootScan setup from
    // `render_oob_section_html_carries_total_audiobooks_for_a_walked_root`.
    let raw = RootScan::Walked {
        canonical_path: PathBuf::from("/lib"),
        folders: vec![ScannedFolder {
            rel_path: PathBuf::from("Book"),
            directly_holds_audio: true,
            missing_ebook: true,
            cover_files: Vec::new(),
            audio_files: vec!["01.mp3".to_string()],
        }],
    };
    let links: Vec<crate::config::SearchLink> = Vec::new();

    let (oob, hash) = rendered_oob_with_hash(&raw, 0, ViewMode::GapsOnly, &links);
    let direct_oob = render_oob_section(&raw, 0, ViewMode::GapsOnly, &links);
    let direct_hash = stable_hash(&direct_oob);

    assert_eq!(oob, direct_oob);
    assert_eq!(hash, direct_hash);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p missing-ebooks --lib autosync::tests::rendered_oob_with_hash_returns_render_oob_section_paired_with_its_stable_hash`

Expected: compile error, "cannot find function `rendered_oob_with_hash` in this scope".

- [ ] **Step 3: Add the helper**

Insert this function in `src/autosync.rs` directly after `render_oob_section` (currently ends at L78):

```rust
/// Render one section's OOB-swap bytes and hash them. Shared by
/// `snapshot_and_seed` and `compute_pushes` so the seed hash and the loop's
/// first-tick hash agree by construction (ADR-0024).
fn rendered_oob_with_hash(
    scan: &crate::scanner::RootScan,
    root_idx: usize,
    mode: ViewMode,
    links: &[crate::config::SearchLink],
) -> (String, u64) {
    let oob = render_oob_section(scan, root_idx, mode, links);
    let hash = stable_hash(&oob);
    (oob, hash)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p missing-ebooks --lib autosync::tests::rendered_oob_with_hash_returns_render_oob_section_paired_with_its_stable_hash`

Expected: PASS.

- [ ] **Step 5: Run the full autosync test module**

Run: `cargo test -p missing-ebooks --lib autosync::tests`

Expected: all autosync tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/autosync.rs
git commit -m "feat(autosync): add rendered_oob_with_hash pair helper

Concentrates render_oob_section + stable_hash into one private helper.
No call sites yet; subsequent commits route snapshot_and_seed and
compute_pushes through it so the seed hash and loop first-tick hash
agree by construction (ADR-0024)."
```

---

### Task 2: Route `snapshot_and_seed` through `rendered_oob_with_hash`

**Files:**
- Modify: `src/autosync.rs::snapshot_and_seed` (L85-98)

**Interfaces:**
- Consumes: `rendered_oob_with_hash` from Task 1.
- Produces: nothing new. `snapshot_and_seed`'s signature and return shape are unchanged.

- [ ] **Step 1: Replace the loop body**

Find this block in `snapshot_and_seed`:

```rust
for (root_idx, section) in raw.iter().enumerate() {
    let oob = render_oob_section(section, root_idx, mode, links);
    hashes.push(stable_hash(&oob));
    payload.push_str(&oob);
}
```

Replace with:

```rust
for (root_idx, section) in raw.iter().enumerate() {
    let (oob, hash) = rendered_oob_with_hash(section, root_idx, mode, links);
    hashes.push(hash);
    payload.push_str(&oob);
}
```

- [ ] **Step 2: Run the autosync test module**

Run: `cargo test -p missing-ebooks --lib autosync::tests`

Expected: all tests pass. Any existing test that exercises `snapshot_and_seed` (search the test module for the symbol) continues to pass byte-for-byte because the helper's contract test in Task 1 pins the pair equality.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test -p missing-ebooks`

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/autosync.rs
git commit -m "refactor(autosync): route snapshot_and_seed through rendered_oob_with_hash

No behavior change. The (oob, hash) pair is now produced by the shared
helper so the seed hash this function writes is structurally equal to
the hash compute_pushes will compute on the next tick."
```

---

### Task 3: Route `compute_pushes` through `rendered_oob_with_hash`

**Files:**
- Modify: `src/autosync.rs::compute_pushes` (L43-50)

**Interfaces:**
- Consumes: `rendered_oob_with_hash` from Task 1.
- Produces: nothing new. `compute_pushes`'s signature and return shape are unchanged.

- [ ] **Step 1: Replace the inner-loop body**

Find this block inside `compute_pushes`:

```rust
for (root_idx, section) in raw.iter().enumerate() {
    let html = render_oob_section(section, root_idx, mode, links);
    let h = stable_hash(&html);
    if last_hash[mode][root_idx] != Some(h) {
        last_hash[mode][root_idx] = Some(h);
        pushes.push((mode, root_idx, html));
    }
}
```

Replace with:

```rust
for (root_idx, section) in raw.iter().enumerate() {
    let (html, h) = rendered_oob_with_hash(section, root_idx, mode, links);
    if last_hash[mode][root_idx] != Some(h) {
        last_hash[mode][root_idx] = Some(h);
        pushes.push((mode, root_idx, html));
    }
}
```

- [ ] **Step 2: Run the autosync test module**

Run: `cargo test -p missing-ebooks --lib autosync::tests`

Expected: all tests pass.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test -p missing-ebooks`

Expected: all tests pass.

- [ ] **Step 4: Verify clippy and fmt are clean**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`

Expected: no output (clean).

- [ ] **Step 5: Commit**

```bash
git add src/autosync.rs
git commit -m "refactor(autosync): route compute_pushes through rendered_oob_with_hash

No behavior change. Both autosync paths now produce the (oob, hash)
pair through one helper, so the ADR-0024 byte-equality invariant
between snapshot-seeded hashes and loop-tick hashes is structural."
```

---

### Task 4: Update architecture-review status

**Files:**
- Modify: `.scratch/architecture-review-2026-06/README.md`

- [ ] **Step 1: Mark candidate #7 done**

In the status table at L9-17, change the `#7` row from:

```
| 7 | Single `RawView::sections(mode)` iterator | Worth exploring | open |
```

to:

```
| 7 | Single `RawView::sections(mode)` iterator | Worth exploring | **done** (scoped down; see `.scratch/autosync-oob-hash-pair/`) |
```

- [ ] **Step 2: Update the "Suggested next pick" footer**

In the closing paragraph at L177-179, drop #7 from the open list. The paragraph currently begins "All three Strong candidates (#1, #2, #3) have shipped. Open candidates are all Worth-exploring or Speculative: #4 (smallest, lands a piece of ADR-0005 in one module), #5 (independent, replaces 31 KB of imperative scenario builders with data), #7 (composes with the just-shipped #3 by collapsing three render loops into one iterator), and #6 (speculative, best paired with #7)."

Replace with: "All three Strong candidates (#1, #2, #3) have shipped, and #7 landed in a scoped-down form (the autosync render+hash pair, not the original `RawView::sections` iterator: post-#3, the iteration shape was already a one-liner and the meaningful remaining duplication was the lockstep inside autosync). Open candidates are #4 (smallest, lands a piece of ADR-0005 in one module), #5 (independent, replaces 31 KB of imperative scenario builders with data), and #6 (speculative)."

- [ ] **Step 3: Commit**

```bash
git add .scratch/architecture-review-2026-06/README.md
git commit -m "docs(arch-review): mark candidate #7 done (scoped down)"
```
