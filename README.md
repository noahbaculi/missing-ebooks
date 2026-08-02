# missing-ebooks

Self-hosted web server that scans your audiobook library to highlight folders that hold audio but no matching ebook, so that gaps can be found and filled.

<!-- TODO: Screenshot -->

## Live demo

Try the live demo with dummy data: **[demo-missing-ebooks.noahbaculi.com](https://demo-missing-ebooks.noahbaculi.com)**.
Each visit opens a private, throwaway sandbox seeded with sample audiobooks. Changes stay in your session and reset when idle.

## Getting started

Docker is the supported distribution path. A multi-arch image (amd64 and arm64) is published to GitHub Container Registry. With [Docker Compose](https://docs.docker.com/compose/), drop this `docker-compose.yml` beside your other stacks, edit the volume and IDs, and run `docker compose up -d`:

Minimal example `docker-compose.yml`. See [docker-compose.yml](../docker-compose.yml) for more detail:

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

      # Must be mounted as a volume so the container can access it
      MISSING_EBOOKS_LIBRARY_ROOTS: /audiobooks

    volumes:
      - /path/to/audiobooks:/audiobooks
      # - ./config.toml:/config/config.toml:ro
    restart: unless-stopped
```

Then open http://127.0.0.1:13379.

- `PUID`/`PGID` set the user the server runs as. The app writes marker files into your library, so set these to match whoever owns it on the host (run `id` to find yours). They default to `1000`.
- The library is mounted read-write at `/audiobooks` and named by `MISSING_EBOOKS_LIBRARY_ROOTS`. For multiple roots, add a mount per root and list the container paths separated by `:`.
- File-only settings (search links, exclude globs, extension lists) come from a mounted `config.toml`. Uncomment the second volume. The entrypoint auto-detects `/config/config.toml`.

> [!WARNING]
> The server has no authentication. It binds to loopback by default, and binding to a non-loopback address logs a warning at startup. To reach it from the LAN, put a reverse proxy with authentication in front of it before exposing it beyond your machine.

## How it works

Point the server at one or more library roots. Each root is scanned and rendered as its own tree. A folder is flagged when it directly holds an audio file and nothing covers it: no ebook and no marker in that folder or any ancestor up to its root. An ebook file or marker covers everything beneath it.

A rescan button forces a cold scan (clears the server cache, walks every directory). Open pages also refresh on their own: while a tab is visible, the client polls a warm rescan every `poll_interval_seconds` and swaps any changed root sections into the page. Scans are cached with a staleness ceiling (`ttl_seconds`) that caps how often the underlying walk runs regardless of how many tabs are open.

### Markers

Two fixed marker files mark a folder as covered without actual ebook files:

| Marker             | Meaning                             |
| ------------------ | ----------------------------------- |
| `.no_ebook`        | No ebook exists or could be sourced |
| `.ebook_elsewhere` | The ebook lives in another folder   |

A marker covers the folder it sits in and everything below it, the same as an ebook. Writing one into a container (an author or series folder) covers every folder under it.

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

| Variable                               | Sets                                     |
| -------------------------------------- | ---------------------------------------- |
| `MISSING_EBOOKS_LIBRARY_ROOTS`         | `library_roots` (OS path-separated list) |
| `MISSING_EBOOKS_BIND`                  | `bind`                                   |
| `MISSING_EBOOKS_PORT`                  | `port`                                   |
| `MISSING_EBOOKS_TTL_SECONDS`           | `ttl_seconds`                            |
| `MISSING_EBOOKS_SCAN_CONCURRENCY`      | `scan_concurrency`                       |
| `MISSING_EBOOKS_POLL_INTERVAL_SECONDS` | `poll_interval_seconds`                  |
| `MISSING_EBOOKS_LOG`                   | Log verbosity (see Logging below)        |
| `PUID`                                 | Container run-as user ID (Docker only)   |
| `PGID`                                 | Container run-as group ID (Docker only)  |

Extension lists, exclude rules, and search links are file-only. The printed template documents every key.

### Logging

`MISSING_EBOOKS_LOG` sets verbosity to one of `error`, `warn`, `info` (the default), `debug`, or `trace`. See [`docs/logging.md`](docs/logging.md) for the per-operation timing detail and the `RUST_LOG` override.

## Network shares

Pointing a library root at an SMB or NFS mount is not encouraged but I understand that many users don't have a choice. The scan is slower than on local disk and scales with the number of folders. Raise `ttl_seconds` and treat the Rescan button as the deliberate refresh. On filesystems with coarse mtime (some NFS and FAT mounts), a change made inside the same mtime tick can be missed until the next cold rescan.

See [`docs/network-shares.md`](docs/network-shares.md) for more details.

## Migration

If you want to clean up from this application, you can simply remove any marker files that may have been written:

```shell
find /path/to/audiobooks \( -name '*.no_ebook' -o -name '*.ebook_elsewhere' \) -delete
```

## License

Released under AGPL-3.0-or-later. See `LICENSE`.
