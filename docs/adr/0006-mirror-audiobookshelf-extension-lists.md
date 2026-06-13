# Detection defaults mirror Audiobookshelf's full extension lists

The default `audio_exts` is Audiobookshelf's full `SupportedAudioTypes` set (20 entries), and the default `ebook_exts` is its full `SupportedEbookTypes` set (6 entries: `epub`, `pdf`, `mobi`, `azw3`, `cbr`, `cbz`). Both stay configurable, but the shipped defaults match ABS rather than a smaller hand-picked set.

We first defaulted to a four-element subset for each list (`mp3`, `m4a`, `m4b`, `flac` and `epub`, `pdf`, `mobi`, `azw3`). That subset covers every format that actually occurs in the author's 8,802-entry library snapshot, with one unused extension in each list. It was a reasonable fit for one library. It was the wrong default for a self-hosted tool other people run.

The deciding factor is that the two lists fail in opposite directions. An under-inclusive audio list produces a silent false negative: a folder whose only audio is `.opus`, `.wav`, or `.aac` (all real ABS audio types) is never flagged, and a missed gap looks identical to a folder that has no gap. For a tool whose whole job is surfacing gaps, that is the worst failure it can have. An under-inclusive ebook list fails the other way: a folder covered by a `.cbz` comic archive gets flagged as missing, which is a false positive the operator can see and correct. So the audio list is what forces the decision, and matching ABS fixes both at once.

The research doc already points the same way: `server/utils/globals.js` is the allowlist that drives ABS detection, "so it is the one to mirror." We verified the two arrays against that file on the `master` branch (2026-06-05) before adopting them.

The cost is small. A wider audio list flags more folders, which is the correct behavior for a gap-finder. Adding `cbr` and `cbz` to the ebook list removes false positives. The author's own library is unaffected, because it holds only formats that were already in the narrower set. A user who wants to narrow detection can still edit the lists in `config.toml`.

ABS maintains these lists (PR #4212 added the `aif` variant, for example), so treat `globals.js` on the tag a user runs as authoritative and re-sync the defaults occasionally rather than assuming they are frozen.
