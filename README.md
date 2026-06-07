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

By default the page shows only the gaps. A "Show all folders" toggle beside the Rescan button switches to a fuller view that renders the whole library tree, covered folders included, so a gap can be read in the context of everything around it. Covered folders show dimmed with a check and carry no actions; the gaps keep their buttons and search links. The toggle is per view and is not saved.

## Getting Started

Set at least one library root and run the server. It exits if no root is configured in any layer.

```shell
MISSING_EBOOKS_LIBRARY_ROOTS="/mnt/nas/Audiobooks" cargo run --release
```

It binds to `127.0.0.1:13379` by default. Open http://127.0.0.1:13379.

> [!NOTE]
> The server has no authentication. It binds to loopback by default; binding to a non-loopback address logs a warning at startup.

## Configuration

Configuration resolves in three layers: built-in defaults, an optional `config.toml`, then environment variables on top (env over file over default).

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

| Variable                       | Sets                                     |
| ------------------------------ | ---------------------------------------- |
| `MISSING_EBOOKS_LIBRARY_ROOTS` | `library_roots` (OS path-separated list) |
| `MISSING_EBOOKS_BIND`          | `bind`                                   |
| `MISSING_EBOOKS_PORT`          | `port`                                   |
| `MISSING_EBOOKS_TTL_SECONDS`   | `ttl_seconds`                            |

Extension lists, exclude rules, and search links are file-only. The printed template documents every key.

## Markers

Two fixed marker files mark a folder as covered on purpose. The names are not configurable.

| Marker             | Meaning                             |
| ------------------ | ----------------------------------- |
| `.no_ebook`        | No ebook exists or could be sourced |
| `.ebook_elsewhere` | The ebook lives in another folder   |

A marker covers the folder it sits in and everything below it, the same as an ebook. Writing one into a container (an author or series folder) covers every folder under it.

## Exploring the UI

`examples/explore.rs` seeds a synthetic library into a temp directory, serves it through the real UI on a loopback port, and tears the directory down on Ctrl-C. Use it to eyeball the rendered output across a catalog of known library states without pointing the server at a real library.

Run a scenario:

```shell
cargo run --example explore -- mixed-forest
```

It prints the URL (the app's default port 13379, or an OS-assigned one if that port is busy) and serves until Ctrl-C. Run with no scenario, or `--help`, to print the catalog:

```shell
cargo run --example explore
```

Scenarios:

| Scenario       | Shows                                                                      |
| -------------- | -------------------------------------------------------------------------- |
| `mixed-forest` | Nested tree: containers, flagged leaves, query cleaning, ancestor coverage |
| `clean-error`  | Two roots side by side: one fully covered (Clean), one uncreated (Error)   |
| `root-flagged` | Loose audio in the root, so the root itself is flagged                     |
| `pre-marked`   | Pre-existing markers hide covered folders; siblings stay click targets     |
| `big-library`  | ~50 authors with mixed coverage and nesting, for testing scroll and layout at volume |

Flags:

| Flag         | Effect                                                     |
| ------------ | ---------------------------------------------------------- |
| `--port N`   | Bind an exact port instead of the default 13379            |
| `--ttl SECS` | Set the scan-cache staleness window (default 0, cache off) |
| `--keep`     | Keep the seeded files on exit and print where they landed  |

> [!NOTE]
> Marker buttons write real `.no_ebook` / `.ebook_elsewhere` files into the seeded tree. Pass `--keep` to inspect them after exit; otherwise the temp directory is removed on shutdown.

For a live-reload loop while iterating on the UI, run `bacon explore` instead. It rebuilds and reruns the harness on a fixed port whenever `src/`, `examples/`, or `assets/` change. The repo pins bacon in `mise.toml`, so `mise install` provisions it.

## Development

After cloning, run `mise install` to provision the pinned tools, then `mise run setup` once to point git at the committed hooks in `.githooks`.

With the hook installed, any commit that touches Rust or build-config files runs `cargo fmt --check` and `cargo clippy` first, the same checks CI enforces, and blocks the commit if either fails. Run them yourself any time with `mise run lint`. Commits that touch only docs skip the build.

## Future Work

- [x] Repeatable UI test harness with the same possible seeded states to test multiple scenarios
- [ ] Prettier UI
- [ ] Tag-based search query built from path structure, not just the leaf folder name
- [ ] Runtime button to append an exclude name and persist it to config
