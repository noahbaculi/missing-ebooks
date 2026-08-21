# Audiobookshelf classification lists we mirror

How Audiobookshelf (ABS, [advplyr/audiobookshelf](https://github.com/advplyr/audiobookshelf)) classifies audio and ebook files by extension. We mirror these lists in `src/config.rs`.

Snapshot as of 2026-06-05, not a maintained reference. Audiobookshelf's parsing may have changed since.

> [!NOTE]
> Sourced from the `master` branch as of 2026-06-05. The lists below have been stable, but a tagged release a user runs could differ. Treat `server/utils/globals.js` in source as the authoritative list rather than the docs site, which doesn't publish a complete extension table.

## How classification works

ABS decides whether a file is audio, ebook, image, text, or metadata purely from its lowercase extension, tested against static allowlist arrays. It doesn't read embedded tags to make this decision. The relevant code:

- `server/utils/globals.js` holds the allowlists: `SupportedAudioTypes`, `SupportedEbookTypes`, `SupportedImageTypes`.
- `server/objects/files/LibraryFile.js` has a `fileType` getter that returns `image`, `audio`, `ebook`, `text`, or `metadata` by testing array membership in order. No tag inspection.
- `server/scanner/LibraryItemScanData.js` filters files with `globals.SupportedAudioTypes.includes(ext.slice(1).toLowerCase())`, which strips the leading dot, lowercases, and checks membership.
- The official [ebooks guide](https://www.audiobookshelf.org/guides/ebooks/) states the rule in plain terms: "Any file with an extension EPUB, PDF, CBR, CBZ, AZW3, MOBI is considered an ebook file."

Classification is case-insensitive and keys only on the extension after the final dot.

## Supported audio extensions

`SupportedAudioTypes` in `globals.js` contains 20 entries:

```
m4b  mp3  m4a  flac  opus  ogg  oga  mp4  aac  wma
aiff aif  wav  webm  webma mka  awb  caf  mpg  mpeg
```

A related map, `AudioMimeType` in `server/utils/constants.js`, has 19 keys. It omits `wav`, which has no MIME entry but is still in the detection list above. `globals.js` is the allowlist that drives detection, so it's the one to mirror.

Expect occasional additions. PR #4212 ("AIFF supported but AIF isn't") added a missing variant.

## Supported ebook extensions

`SupportedEbookTypes` in `globals.js` contains exactly six entries:

```
epub  pdf  mobi  azw3  cbr  cbz
```

The [server FAQ](https://www.audiobookshelf.org/faq/server/) confirms the same set: "An ebook file can be a PDF, AZW3, MOBI, EPUB, CBR, CBZ."

`.fb2` and `.txt` aren't supported as ebooks. They appear only in user feature requests. `.txt` and `.nfo` are classified as text files, and `.opf`, `.abs`, `.xml`, `.json` are classified as metadata files, all handled separately from ebooks.

`.cbr` and `.cbz` are comic-book archives. ABS counts them as ebooks even though they aren't text formats.

> `azw3` and `mobi` have limited in-app reader support and don't save reading progress, but they're still classified as ebooks by extension, so detection is unaffected.

## Library and folder structure

ABS builds books from the directory tree, described in the [book scanner guide](https://www.audiobookshelf.org/guides/book-scanner/):

- Each audio or ebook file sitting loose in the root of a library folder is treated as its own book.
- Any subfolder that contains at least one supported audio or ebook file is treated as one book, including nested subfolders.
- An `Author/Series/Title` nesting is supported. From the folder names, ABS can parse title, subtitle, asin, authors, narrators, series, series sequence, and published year.

> ABS consolidates the media inside a book folder into a single book at the deepest book-directory level. If a title splits its audio across `CD1` and `CD2` subfolders, ABS still reports one book, not two.

## Ignored and non-content files

These never count as audio or ebook content:

- Cover images (the `SupportedImageTypes` list: `png`, `jpg`, `jpeg`, `webp`, verified in `globals.js` on `master`, 2026-06-05)
- Text sidecars: `.txt`, `.nfo`
- Metadata sidecars: `.opf`, `metadata.json`, `metadata.abs`, plus `.xml` and `.json`

A `.opf` is metadata, not an ebook. A folder whose only book-like file is a `.opf` has no ebook as far as ABS is concerned.

## Files and directories skipped before classification

Classification by extension is the second stage. Before it, ABS drops some entries from the scan entirely, so they're never classified at all. The rule lives in `shouldIgnoreFile` in `server/utils/fileUtils.js`, verified on `master` (2026-06-05):

```text
if basename starts with ".", ignore as "dotfile"
if any path segment starts with ".", ignore as "dotpath"
```

The first check drops any file whose own name starts with a dot. The second drops any file sitting under a directory whose name starts with a dot. So a macOS AppleDouble shadow such as `._The Martian.epub` is skipped by the dotfile rule: it carries an `.epub` extension but never reaches classification. Everything inside a `.git`, `.@__thumb`, or `.stfolder` directory is skipped by the dotpath rule. ABS has no separate AppleDouble handling. The leading dot is what catches those files.

One vendor name is hard-coded alongside the dot rules:

```text
includeAnywhereIgnore = ["@eaDir"]
```

`@eaDir` is a Synology thumbnail directory. It doesn't start with a dot, so the dotpath rule misses it, and ABS names it explicitly. No other vendor name (`#recycle`, `Thumbs.db`) is hard-coded. Anything starting with a dot is already covered.

## Source of truth and version notes

- Authoritative lists: `server/utils/globals.js` on the branch or tag you run
- Classification getters: `server/objects/files/LibraryFile.js`, `server/scanner/LibraryItemScanData.js`
- Docs: [book scanner](https://www.audiobookshelf.org/guides/book-scanner/), [ebooks](https://www.audiobookshelf.org/guides/ebooks/), [server FAQ](https://www.audiobookshelf.org/faq/server/)

The docs page lists ebook formats as "epub, pdf, cbr, cbz" in passing and mentions a few audio extensions, but doesn't enumerate the full audio set. Use the source files when syncing extension lists.
