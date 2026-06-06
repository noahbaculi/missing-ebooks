# missing-ebooks

Self-hosted web server that scans audiobook library trees and surfaces folders that hold audio but no matching ebook, so the gaps are easy to find and fill.

## How It Works

Point the server at one or more library roots. Each root is scanned and rendered as its own tree. A folder is flagged when it directly holds an audio file and nothing covers it: no ebook and no marker in that folder or any ancestor up to its root. One ebook or marker covers everything beneath it.

```
Audiobooks/
  Andy Weir/
    Artemis/                          flagged (audio, no ebook)
    The Martian/                      flagged (audio, no ebook)
  Cixin Liu/
    Remembrance of Earth's Past/      covered by a series-level epub
      1 - The Three-Body Problem/
      2 - The Dark Forest/
```

Each flagged row carries:

- buttons that write a marker file into that folder
- search links that open a prefilled book search in a new tab (Goodreads and OceanofPDF by default)

A rescan button refreshes the view. Scans are cached with a staleness backstop (`ttl_seconds`).

## Getting Started

Set at least one library root and run the server. It exits if no root is configured in any layer.

```shell
MISSING_EBOOKS_LIBRARY_ROOTS="/mnt/nas/Audiobooks" cargo run --release
```

It binds to `127.0.0.1:8080` by default. Open http://127.0.0.1:8080.

> [!NOTE]
> The server has no authentication. It binds to loopback by default (ADR-0003); binding to a non-loopback address logs a warning at startup.

## Configuration

Configuration resolves in three layers: built-in defaults, an optional `config.toml`, then environment variables on top (env over file over default, ADR-0004).

Print the annotated template:

```shell
cargo run -- --print-config > config.toml
```

Run with a specific file:

```shell
cargo run -- --config config.toml
```

A minimal `config.toml`:

```toml
library_roots = ["/mnt/nas/Audiobooks"]
exclude_globs = ["**/*(abridged)*"]
```

These environment variables override the file when set:

| Variable | Sets |
| --- | --- |
| `MISSING_EBOOKS_LIBRARY_ROOTS` | `library_roots` (OS path-separated list) |
| `MISSING_EBOOKS_BIND` | `bind` |
| `MISSING_EBOOKS_PORT` | `port` |
| `MISSING_EBOOKS_TTL_SECONDS` | `ttl_seconds` |

Extension lists, exclude rules, and search links are file-only. The printed template documents every key.

## Markers

Two fixed marker files mark a folder as covered on purpose. The names are not configurable, so detection and the write buttons can never drift apart.

| Marker | Meaning |
| --- | --- |
| `.no_ebook` | No ebook exists or could be sourced |
| `.ebook_elsewhere` | The ebook lives in another folder |

A marker covers the folder it sits in and everything below it, the same as an ebook. Writing one into a container (an author or series folder) covers every folder under it.

## Future Work

- [ ] Tag-based search query built from path structure, not just the leaf folder name (ADR-0010)
- [ ] Runtime button to append an exclude name and persist it to config (ADR-0002)
