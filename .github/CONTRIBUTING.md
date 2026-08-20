# Contributing

This repo is a single-author hobby project, so the contribution surface is small: open an issue first for anything non-trivial, and keep changes scoped and well-explained.

The domain glossary lives at [`docs/CONTEXT.md`](../docs/CONTEXT.md) and defines common terminology for this project.

## Terminology

Use `ebook` for the general object: `missing ebook`, `ebook file`, `ebooks and markers`. Use `Ebook` only at the start of a sentence or where title case is required. Use `audiobook` as one word, `EPUB` for the file format, and `Books` only when naming a library category. Do not use `eBook` or `e-book` in project copy unless quoting another product or source.

Marker filenames stay lowercase and underscored on disk: `.no_ebook` and `.ebook_elsewhere`.

## Dev setup

With the exception of [Rust](https://rust-lang.org/), development dependencies are managed by [`mise`](https://github.com/jdx/mise) via the `mise.toml` file. `mise install` provisions the pinned tools.

The committed `.githooks/pre-commit` runs `cargo fmt`, `cargo clippy`, `cargo doc -D warnings`, and (for asset or accent-test changes) `mise run test:accent`. `mise.toml`'s `[hooks] enter` entry auto-activates the hook on the first `cd` into the worktree. Contributors who do not use mise shell integration run `mise run setup` once per clone (and once per worktree) to point git at the same hooks.

Git shares `core.hooksPath` from the main `.git/config` across worktrees, so the first worktree's `enter` hook writes the value and every other worktree's idempotent guard short-circuits. `mise run setup` stays the manual fallback for clones without mise shell integration loaded.

Alternatives rejected: `cargo-husky` adds a dev-dependency and does not fire until tests run; a `build.rs` side effect pollutes a build script with non-build behavior and skips the docs-only commit path; manual-only onboarding relies on humans reading instructions, which is exactly the failure mode the data showed. Revisit when mise shell integration stops being a reasonable baseline, or when the hook grows expensive enough to want a framework's parallelism or watcher features.

Never bypass the hook with `--no-verify`. The hook runs the same checks CI enforces. Bypassing them just moves the failure to the CI run.

GitHub Actions in `.github/workflows/` are pinned to a full 40-character commit SHA with a trailing `# vX.Y.Z` version comment (e.g., `uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0`). Dependabot's `github-actions` ecosystem entry refreshes the SHAs weekly and updates the version comment in the same PR. Do not introduce floating tag refs (`@v4`, `@main`); the supply-chain trail the release workflow publishes (SBOM + SLSA provenance) is only as strong as its weakest ref.

### Client JS type checking

`assets/app.js` and `assets/prepaint.js` are plain JavaScript with `// @ts-check` and JSDoc annotations. A check-only TypeScript pass (`tools/tsconfig.json` with `checkJs` and `noEmit` under `strict`) reads them. The htmx surface and app-custom events are typed by a hand-written ambient stub at `tools/htmx.d.ts`. There is no `package.json`, no lockfile, no `node_modules`, and nothing is emitted: the source stays the shipped artifact. The check is pinned through mise and runs in the pre-commit hook and CI. If the client ever grows into multiple modules that need bundling, a real build step becomes worth its weight and full TypeScript with emitted output would carry it; until then the check-only pass is the cheapest way to keep the surface honest.

## Build and test

MSRV is Rust 1.97 (matches `rust-toolchain.toml`).

MSRV tracks the latest stable Rust release. When a new stable ships, bump all four pinned locations in one commit. The `toolchain-drift` CI job enforces that they agree.

- `Dockerfile`: `FROM rust:X.Y.Z-alpine@sha256:...`, digest from the Dependabot base-image PR
- `rust-toolchain.toml`: `channel = "X.Y.Z"`
- `Cargo.toml`: `rust-version = "X.Y"`
- `.github/workflows/ci.yml`: two `X.Y.0` references in the `msrv` job

Dependabot's `docker` ecosystem opens the base-image PR that starts each bump.

`mise tasks` lists every check. The shortcuts most reached for:

- `mise run check` runs the full pre-commit equivalent.
- `mise run test` runs nextest plus doctests.
- `mise run lint` runs fmt, clippy, typos, taplo, and the unused-dep check.

HTTP-reachable `tokio::spawn` sites must fold `JoinError` into a real error path. See `spawn_mutation` in `src/state.rs` for the pattern.

## Exploring the UI

The UI harness seeds a synthetic library into a temp directory and serves the production router against it.
This harness is used to eyeball the rendered output across a catalog of known library states without pointing the server at a real library.

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
| `pre-marked`   | Pre-existing markers hide covered folders. Siblings stay click targets                                                                                          |
| `big-library`  | ~50 authors with mixed coverage and nesting, for testing scroll and layout at volume                                                                            |

Flags:

| Flag          | Effect                                                                                                                  |
| ------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `--port N`    | Bind an exact port instead of the default 13379                                                                         |
| `--ttl SECS`  | Set the scan-cache staleness window (default 0, cache off)                                                              |
| `--keep`      | Keep the seeded files on exit and print where they landed                                                               |
| `--bind <IP>` | Bind a specific address instead of `127.0.0.1`. Non-loopback binds skip the preferred-port fallback and warn on stderr. |

> [!NOTE]
> Marker buttons write real `.no_ebook` / `.ebook_elsewhere` files into the seeded tree. Pass `--keep` to inspect them after exit. Otherwise the temp directory is removed on shutdown.

For a live-reload loop while iterating on the UI, run `bacon explore` instead. It rebuilds and reruns the harness on a fixed port whenever `src/` or `assets/` change.

## Running against a real library (Docker)

`dev/docker-compose.yml` builds the local `missing-ebooks:dev` image, mounts a host path as `/audiobooks`, and layers `dev/config.toml` at `/config/config.toml`. The image auto-detects that path (`Dockerfile` sets `MISSING_EBOOKS_CONFIG=/config/config.toml`), so no extra env is needed.

Build and start:

```shell
docker build -t missing-ebooks:dev . && docker compose -f dev/docker-compose.yml up -d --force-recreate
```

Then open http://localhost:13379/.

Stop and remove:

```shell
docker compose -f dev/docker-compose.yml down
```

Point the `/audiobooks` mount at your library by editing `dev/docker-compose.yml`. `dev/config.toml` is only read by this compose file and stays out of the release images.

## Commit style

Conventional Commits: `type(scope): subject`. A `feat` or `fix` carries a body explaining the why and the effect, with a scope caveat where one applies (`No behavior change.`, `Prose only: ...`). A trivial change (a one-line doc edit, an ADR record) can be subject-only.

Work lands on `main` by rebase and fast-forward only, so each commit sits inline on a linear history with no merge commit above it to carry context. Keep commits granular and independent.

## Where work lives

- Issues, PRDs, and implementation plans live under `.scratch/<feature>/`, gitignored by default.
- ADRs live at `docs/adr/NNNN-kebab-title.md`. The template and amendment convention are in [`docs/adr/README.md`](../docs/adr/README.md).

## PR expectations

Open an issue first for non-trivial work so the design conversation happens before code does. Keep commits granular. Each one reads on its own and passes CI on its own. Include the user-facing why in the commit body.

If you change anything that renders in the UI (HTML in `src/web.rs`, styles in `assets/app.css`, behavior in `assets/app.js`), include the UI harness command you used to eyeball it (e.g. `cargo run --example explore -- mixed-forest`).
