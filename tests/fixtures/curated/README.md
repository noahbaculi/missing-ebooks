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
| `Brandon Sanderson/The Mistborn Saga/4.5 - Allomancer Jak and the Pits of Eltania` | covered | `.ebook_elsewhere` marker beside the audio (in-folder coverage, the real shape this marker takes in the snapshot) |
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
| `Dresden Files/Dead Beat` | flagged | a `.cue` sheet beside the audio is not an ebook |
| `Becky Chambers/Wayfarers/4 - The Galaxy, and the Ground Within` | flagged | a hidden `.beets` sidecar (its name embeds `.mp3`) is ignored |
| `Robin Hobb/Farseer Trilogy/1 - Assassin’s Apprentice` | flagged | `[ebook]` in a `.jpg` name is not an ebook; non-ASCII apostrophe (U+2019) |
| `Robin Hobb/_Extras` | absent | maps-only folder, no audio |
| `Arthur C. Clarke/Rendezvous with Rama` | flagged | a sibling `_more_ebooks` stash does not cover it |
| `Margaret Atwood/The Handmaid's Tale/1 - The Handmaid's Tale` | flagged | two audio formats (`.m4b` + `.mp3`) reported together |
| `Michael J. Sullivan/Riyria Revelations` | absent | empty subtree (`.gitkeep` only), no audio |
| `Christopher Paolini/Inheritance Cycle (Abridged)` | excluded | `**/*(abridged)*` glob prunes the subtree (ADR-0001) |
| `missing_ebooks.txt` (root) | ignored | the reference tool's own output file |

The verdicts follow the rules in [`CONTEXT.md`](../../../CONTEXT.md) and are the
contract the scanner must satisfy. When the scanner changes a rule, update
`expected.json` in the same change.

`expected.json` also carries three fields beyond the per-folder verdicts.
`config` is the configuration the whole expected output assumes (the abridged
exclude glob). `excluded` lists folders an exclusion rule drops, pruning their
subtree. `notes` holds assertions that are not tied to a single folder, such as
the root output file being ignored, and a record that the ancestor-coverage and
AppleDouble-only-ebook cases are synthetic: the real snapshot has no such
instance, so they defend a spec rule rather than reproduce observed data. A
helper, `validate_expected.py`, checks that `expected.json` and the tree stay
consistent.
