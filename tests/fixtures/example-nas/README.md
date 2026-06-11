# example-nas library snapshot

A frozen, machine-readable listing of a real audiobook tree on a NAS, so the scanner can be developed and tested on a machine that has no access to the mount.

## What is here

| File | Purpose |
| --- | --- |
| `audiobooks.snapshot` | The structural dump. One line per entry, `<type>\t<relative-path>`, sorted with `LC_ALL=C`. `type` is `d` for directory or `f` for file. Paths are relative to the library root. |
| `rehydrate.sh` | Replays the snapshot into a tree of empty files so the scanner can walk a real directory structure. |
| `README.md` | This file. |

The snapshot holds names and extensions only, no file contents. Coverage logic keys off which files exist and what their extensions are (see [`CONTEXT.md`](../../../CONTEXT.md)), so empty files are enough to exercise it.

For a limited-context reader: this README, with the counts under "Structure and quirks" below, is enough to understand the tree's shape. You do not need to load `audiobooks.snapshot` (700 KB+) to reason about it. For small, hand-checkable cases with known expected verdicts, use [`../curated/`](../curated/) instead.

## Where it came from

- Source mount: a CIFS/SMB share mounted at `/mnt/example-nas` (host and share name scrubbed).
- Library root: `/mnt/example-nas/Entertainment/Audiobooks`.
- Captured: 2026-06-04. 126 top-level entries, 900 directories, 7,902 files.

To regenerate from the NAS:

```bash
cd /mnt/example-nas/Entertainment/Audiobooks
find . -mindepth 1 -printf '%y\t%P\n' | LC_ALL=C sort -t$'\t' -k2 > audiobooks.snapshot
```

To rebuild a walkable tree from the snapshot:

```bash
./rehydrate.sh            # builds ./rehydrated
./rehydrate.sh /tmp/lib   # or a target dir of your choosing
```

## Structure and quirks

The common shape is `Author / [Series] / Book / files`, but the real tree breaks that pattern often enough that the scanner has to handle the exceptions. Mapped to the `CONTEXT.md` vocabulary:

- The library root is `Audiobooks`. Every reported folder lives under it.
- Containers are usually author folders, and sometimes a series folder one level down (for example `Brandon Sanderson/The Mistborn Saga`). Some series sit at the top level with no author above them, such as `Dresden Files` and `Legends of the First Empire`, so a top-level entry is not always an author.
- Co-author folders carry a comma-joined name, such as `Frank Herbert, Bill Ransom` and `Robert Jordan, Brandon Sanderson`. There are 15 of these at the top level. A comma in a folder name is normal data, not a delimiter to split on.
- Book folders hold the audio plus side files: `cover.jpg`, `desc.txt`, `reader.txt`, and the occasional `.webp` map. Audio is mostly `.m4b` (3,317) and `.mp3` (2,217), with some `.m4a` (43).
- Covered folders carry the ebook beside the audio. There are 430 ebooks: 415 `.epub`, 13 `.pdf`, 2 `.mobi`. A book with an `.epub` next to its `.m4b` is the typical covered case.
- Markers already exist in the tree, 6 of them. Both kinds appear, and they sit at different depths. `.no_ebook` shows up at the book-folder level (for example `Orson Scott Card/Space Boy/.no_ebook`) and `.ebook_elsewhere` shows up at both book and series level (for example `Ursula K. Le Guin/The Earthsea Trilogy/.ebook_elsewhere`).
- AppleDouble noise is present: 35 files named `._*`, written by macOS over SMB (for example `Andy Weir/Artemis/._01 - Artemis (Unabridged).mp3`). These shadow real files and must not be counted as audio or ebooks. The leading `._` is the thing to filter on.

Because the source is an SMB share, the tree has no Unix symlinks, dotfiles are visible, and the AppleDouble files above are present. A scanner tested only against a clean local directory would miss all three.
