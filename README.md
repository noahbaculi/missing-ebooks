# missing-ebooks

Self-hosted web server that scans audiobook library trees and surfaces folders that hold audio but no matching ebook, so the gaps can be found and filled.

## Live demo

Try it without installing anything: **[demo-missing-ebooks.noahbaculi.com](https://demo-missing-ebooks.noahbaculi.com)**.
Each visit opens a private, throwaway sandbox seeded with sample audiobooks. Changes stay in your session and reset when idle.

## How it works

Point the server at one or more library roots. Each root is scanned and rendered as its own tree. A folder is flagged when it directly holds an audio file and nothing covers it: no ebook and no marker in that folder or any ancestor up to its root. One ebook or marker covers everything beneath it.

A strip above the tree reads `{pct}% covered · {covered} of {total} audiobooks`, where an audiobook is a folder that directly holds audio. The readout updates live as gaps are marked and excludes errored roots from both sides of the ratio.

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

A rescan button forces a cold scan (clears the dir index, walks every directory). With `autosync_interval_seconds > 0`, open pages also refresh live as the background warm scan detects changes. Scans are cached with a staleness backstop (`ttl_seconds`).

By default the page shows only the gaps. A "Show all folders" toggle beside the Rescan button switches to a fuller view that renders the whole library tree, covered folders included, so a gap can be read in the context of everything around it. Covered folders show dimmed with a check and carry no actions; the gaps keep their buttons and search links. The toggle is per view and is not saved.

## Getting started

Set at least one library root and run the server. It exits if no root is configured in any layer.

```shell
MISSING_EBOOKS_LIBRARY_ROOTS="/mnt/nas/Audiobooks" cargo run --release
```

It binds to `127.0.0.1:13379` by default. Open http://127.0.0.1:13379.

> [!NOTE]
> The server has no authentication. It binds to loopback by default; binding to a non-loopback address logs a warning at startup.

## Run with Docker

A multi-arch image (amd64 and arm64) is published to GitHub Container Registry. With [Docker Compose](https://docs.docker.com/compose/), drop this `docker-compose.yml` beside your other stacks, edit the volume and IDs, and run `docker compose up -d`:

```yaml
services:
  missing-ebooks:
    image: ghcr.io/noahbaculi/missing-ebooks:latest
    container_name: missing-ebooks
    ports:
      - "127.0.0.1:13379:13379"
    environment:
      PUID: 1000
      PGID: 1000
      MISSING_EBOOKS_LIBRARY_ROOTS: /audiobooks
    volumes:
      - /mnt/nas/Audiobooks:/audiobooks
      # - ./config.toml:/config/config.toml:ro
    restart: unless-stopped
```

Then open http://127.0.0.1:13379.

- `PUID`/`PGID` set the user the server runs as. The app writes marker files into your library, so set these to match whoever owns it on the host (run `id` to find yours). They default to `1000`.
- The library is mounted read-write at `/audiobooks` and named by `MISSING_EBOOKS_LIBRARY_ROOTS`. For multiple roots, add a mount per root and list the container paths separated by `:`.
- File-only settings (search links, exclude globs, extension lists) come from a mounted `config.toml`. Uncomment the second volume; the entrypoint auto-detects `/config/config.toml`.

> [!WARNING]
> The app has no authentication. The compose file above binds the host port to `127.0.0.1`, so it is reachable only from the machine running it. To reach it from the LAN, change the mapping to `"13379:13379"`, and put a reverse proxy with authentication in front of it before exposing it beyond your network.
>
> The `/events` SSE endpoint serves long-lived `text/event-stream` connections with a 15-second keepalive. A reverse proxy in front must not buffer this path and must hold the connection open past the keepalive interval. For nginx, that means `proxy_buffering off;` and `proxy_read_timeout 60s;` (or longer) on the `/events` location, plus passing the `X-Accel-Buffering: no` response header through. The server sets that header itself, so a proxy that respects it needs no extra config.

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

| Variable                          | Sets                                     |
| --------------------------------- | ---------------------------------------- |
| `MISSING_EBOOKS_LIBRARY_ROOTS`    | `library_roots` (OS path-separated list) |
| `MISSING_EBOOKS_BIND`             | `bind`                                   |
| `MISSING_EBOOKS_PORT`             | `port`                                   |
| `MISSING_EBOOKS_TTL_SECONDS`      | `ttl_seconds`                            |
| `MISSING_EBOOKS_SCAN_CONCURRENCY` | `scan_concurrency`                       |
| `MISSING_EBOOKS_AUTOSYNC_INTERVAL_SECONDS` | `autosync_interval_seconds`              |
| `PUID`                            | Container run-as user ID (Docker only)   |
| `PGID`                            | Container run-as group ID (Docker only)  |

Extension lists, exclude rules, and search links are file-only. The printed template documents every key.

### Logging

Set `MISSING_EBOOKS_LOG` to control verbosity: `error`, `warn`, `info` (the default), `debug`, or `trace`. Raising it to `debug` or `trace` is scoped to this app, so the dependencies stay quiet; lowering it to `warn` or `error` quiets everything to that level. `debug` adds per-operation timings (per-root scans, cache hits and misses, marker writes, and request and render latency), the level to run when checking how a real library performs. `trace` adds a line per directory walked. For full control, set `RUST_LOG` to override it with standard `tracing` filter syntax, for example `RUST_LOG=missing_ebooks::scanner=trace`.

## Network shares

Pointing a library root at an SMB or NFS mount is supported and common, but the scan is far slower than on local disk and there is a firm limit on how much that can be sped up. The walk reads every folder, and each folder costs a handful of round trips to open, list, and close it, with none per file, so scan time scales with the number of folders rather than the number of files.

The largest lever is where the server runs. On the machine that holds the library, when that is an option, the scan is far faster: at the default concurrency a benchmark of one ~900-folder library scanned in tens of milliseconds on its local disk against about two seconds over an SMB mount of the same library, and the gap widens as the library grows.

Reading more folders at once with `scan_concurrency` overlaps their round trips. On local storage that is a large win, roughly sevenfold at the default. Over SMB it is close to a no-op: the server answers one connection's requests in order, so the extra readers fold back onto one and the walk gains only about a third (see [ADR-0019](docs/adr/0019-scan-walk-parallel-sized-by-concurrency.md)). Set the value by the speed of your NAS, not your CPU count; the readers spend almost all their time waiting on the network, so they cost little CPU even well above the core count, and raising a container's `--cpus` does not help.

`ttl_seconds` keeps a scanned view cached so repeat page loads do not rescan, and this matters more over SMB than locally. The client's own attribute cache ages out within a second, faster than a multi-second walk finishes, so a second walk re-queries the server and runs no faster than the first; the in-process cache is what spares the repeat cost. Raise `ttl_seconds` on a slow mount and treat the rescan button as the deliberate refresh.

`autosync_interval_seconds` (default 10) governs the background sync loop. While at least one browser tab is open to the server, the loop runs a warm scan every N seconds (idle gap: the timer measures from completion to next start) and pushes any changed root sections back to the tab over SSE. Warm scans reuse a per-directory mtime index built up by previous scans; on the README's ~900-folder reference library a steady-state warm scan finishes in low single-digit milliseconds over SMB. Set the value to `0` to disable the loop; the SSE endpoint still serves the initial snapshot but emits no further section events. The Rescan button takes a different path: it clears the dir index and walks every directory (a cold scan), the explicit "fix any drift" action, which on the same library is about 1.9 s over SMB.

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

| Scenario       | Shows                                                                                                                                                           |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `mixed-forest` | Three roots: a nested showcase forest, a smaller forest with cross-root `.ebook_elsewhere` markers, and a fully covered Clean root                              |
| `messy-shelf`  | Inconsistent organization: standalone books, author-only/series-only folders, a half-sorted author, a dumping folder, beside one tidy author>series>book pocket |
| `clean-error`  | Two roots side by side: one fully covered (Clean), one uncreated (Error)                                                                                        |
| `root-flagged` | Loose audio in the root, so the root itself is flagged                                                                                                          |
| `pre-marked`   | Pre-existing markers hide covered folders; siblings stay click targets                                                                                          |
| `big-library`  | ~50 authors with mixed coverage and nesting, for testing scroll and layout at volume                                                                            |

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

After cloning, run `mise install` to provision the pinned tools. With mise's shell integration active, the next time you cd into the repo `core.hooksPath` is set to `.githooks` automatically, so the committed pre-commit hook runs without further setup. Contributors who do not use mise shell integration can run `mise run setup` once per clone (and once per worktree) to point git at the same hooks.

With the hook active, any commit that touches Rust or build-config files runs `cargo fmt`, `cargo clippy`, and `cargo doc -D warnings` first, and any commit that touches `assets/app.{css,js}` or `tests/accent/` runs `mise run test:accent`. These are the same checks CI enforces, run locally so the failures surface before push. Run them yourself any time with `mise run lint` (fmt and clippy) or by hand. Commits that touch only docs skip every check.
