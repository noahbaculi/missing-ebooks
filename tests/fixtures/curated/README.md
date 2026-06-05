# curated fixture

A small, hand-built audiobook tree distilled from
[`../example-nas/`](../example-nas/), for fast unit tests with known expected
verdicts. Where `example-nas` is the full real library (8,802 entries) used for
golden-output and scale tests, this fixture is tiny enough to read by eye and
verify by hand.

## Layout

- `Audiobooks/` is the library root the scanner points at. These are real (empty)
  files, so tests walk the tree directly with no rehydrate step.
- `expected.json` is the expected findings: every flagged folder, covered folder,
  container, and notable absence, each with the reason. **Read that file alone to
  know the full expected output. You do not need to walk the tree or open the
  example-nas snapshot.**

## Cases covered

| Folder | Verdict | Pattern it exercises |
| --- | --- | --- |
| `Adrian Tchaikovsky/Cage of Souls` | covered | ebook (`.epub`) beside the audio, the typical covered case |
| `Adrian Tchaikovsky/Elder Race` | flagged | audio (`.mp3`), no ebook |
| `Brandon Sanderson/The Mistborn Saga/Mistborn 01 - The Final Empire` | flagged | deep author / series / book nesting |
| `Orson Scott Card/Space Boy` | covered | `.no_ebook` marker in the folder |
| `Ursula K. Le Guin/The Earthsea Trilogy/*` | covered | `.ebook_elsewhere` marker one level up (ancestor coverage) |
| `Frank Herbert, Bill Ransom/The Jesus Incident` | covered | comma in the author name is data, not a delimiter |
| `Dresden Files/01 - Storm Front` | flagged | series at the top level, no author above |
| `Andy Weir/Artemis` | flagged | AppleDouble `._` files present and ignored |
| `Andy Weir/The Martian` | flagged | the only `.epub` is an AppleDouble shadow and must not count |
| `Neal Stephenson/Cryptonomicon` | covered | `.pdf` counts as an ebook |
| `Neal Stephenson/Snow Crash` | covered | `.mobi` counts as an ebook |
| `Cixin Liu/Remembrance of Earth's Past/*` | covered | one ebook at the collection level covers everything beneath |
| `Becky Chambers/A Psalm for the Wild-Built` | flagged | `.m4a` counts as audio |
| `Unsorted` | absent | no audio anywhere, never flagged |

The verdicts follow the rules in [`CONTEXT.md`](../../../CONTEXT.md) and are the
contract the scanner must satisfy. When the scanner changes a rule, update
`expected.json` in the same change.
