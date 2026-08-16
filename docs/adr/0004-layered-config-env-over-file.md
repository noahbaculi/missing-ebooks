# Config is layered: env > file > default, with the file optional

Date: 2026-06-04.

## Context

This reverses the original spec's "a single config.toml drives the server."

## Decision

Configuration resolves from three layers, most-specific first: environment variable, then an optional `config.toml`, then a built-in default. Command-line flags, if added later, sit above env; `--config` only locates the file and is outside the value chain.

Every setting except `library_roots` has a built-in default (the extension lists, the search links, `bind`, `port`, `scan_cache_ttl_seconds`), so the file is optional. The per-deployment knobs are also settable via prefixed env vars (`MISSING_EBOOKS_LIBRARY_ROOTS`, `_BIND`, `_PORT`, `_SCAN_CACHE_TTL_SECONDS`); the prefix avoids collisions, notably with the common bare `PORT`. The structured config stays file-only: `search_links` (a list of tables) plus the extension lists, `exclude_globs`, and `excluded_dirs`, because lists and nested tables encode badly in env vars.

`library_roots` is required; when it is absent from every layer the server prints how to set it and exits non-zero. There is no auto-write-and-exit on first run (awkward in containers): `--print-config` emits the documented, commented template on demand instead, which is also how a user discovers the file-only structured settings.

## Consequences

The reasoning: most self-hosted apps deployed via Docker are env-first (Miniflux, Paperless-ngx, Audiobookshelf), and `env > file > default` is the near-universal precedence (Viper, twelve-factor, Spring Boot), while file-beats-env surprises Compose users. A plain Docker deployment sets one env var (`MISSING_EBOOKS_LIBRARY_ROOTS`) and mounts the library read-write, with no config file at all. Power users mount a `config.toml` for custom search links or globs. Bare-metal users use env, a file, or `--print-config`.
