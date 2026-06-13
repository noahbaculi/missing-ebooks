# The unit of work is the folder, not the book

The tool reports folders. Each folder is judged on one local question: does it directly hold audio with no ebook or marker covering it. The tool never groups files into a "book" and never tries to find where one book ends and the next begins.

Audiobookshelf, the reference point for our file classification, uses a different unit. Its unit is the book. ABS is file-granular at a library root (each loose file there becomes its own book) and folder-granular below it, and it consolidates audio split across disc subfolders such as `CD1` and `CD2` into one book at the deepest book directory. Our classification of individual files mirrors ABS exactly (see ADR-0006 and `docs/research/audiobookshelf-parsing.md`). The unit of aggregation is where the two part ways, and that is on purpose.

Three behaviors fall out of choosing the folder as the unit:

- Loose audio sitting directly in a library root flags the root folder once, rather than producing one finding per file. ADR-0005 worked this out for the root case; it is the same rule applied at the top of the tree.
- A book split across `CD1` and `CD2` with no ebook flags each subfolder. The parent book folder then renders as a container, and one marker on it covers both children through ancestor coverage. The result is finer-grained than ABS but resolves to the same single action.
- An ebook or marker covers every folder beneath it, up to and including the root. ABS has no equivalent: each book is independent, so an ebook at a series folder level becomes its own ebook-only book and does not cover the volumes under it. Our rule matches the reference Python script and matches how a person reads a shelf, where one series-level ebook means the series is covered.

We chose the folder over the book because the folder question is layout-agnostic. It needs no heuristic for where a book starts, no disc-folder detection, and no parsing of an `Author/Series/Title` convention that not every library follows. It mirrors the reference script's `os.walk`-with-prune directly. The price is that the tool can show more rows than ABS would show books, but container marking folds that back into one action.

The disc-subfolder case does not occur in the author's library: the snapshot has no `CD`, `Disc`, or `Part` folders, and no audio-bearing folder sits above another. So we build no consolidation. If the case became common, the existing container and ancestor-coverage behavior already absorbs it without a new code path.
