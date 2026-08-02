# Contributing

This repo is a single-author hobby project, so the contribution surface is small: open an issue first for anything non-trivial, and keep changes scoped and well-explained.

The domain glossary lives at [`CONTEXT.md`](CONTEXT.md) in the repo root and defines common terminology for this project.

## Dev setup

With the exception of [Rust](https://rust-lang.org/), development dependencies are managed by [`mise`](https://github.com/jdx/mise) via the `mise.toml` file. `mise install` provisions the pinned tools.

The committed `.githooks/pre-commit` runs `cargo fmt`, `cargo clippy`, `cargo doc -D warnings`, and (for asset or accent-test changes) `mise run test:accent`. `mise.toml`'s `[hooks] enter` entry auto-activates the hook on the first `cd` into the worktree (see [ADR-0026](docs/adr/0026-pre-commit-hook-auto-activation.md)). Contributors who do not use mise shell integration run `mise run setup` once per clone (and once per worktree) to point git at the same hooks.

Never bypass the hook with `--no-verify`. The hook runs the same checks CI enforces. Bypassing them just moves the failure to the CI run.

## Build and test

MSRV is Rust 1.96 (matches `rust-toolchain.toml`).

`mise tasks` lists every check. The shortcuts most reached for:

- `mise run check` runs the full pre-commit equivalent.
- `mise run test` runs nextest plus doctests.
- `mise run lint` runs fmt, clippy, typos, taplo, and the unused-dep check.

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

| Flag         | Effect                                                     |
| ------------ | ---------------------------------------------------------- |
| `--port N`   | Bind an exact port instead of the default 13379            |
| `--ttl SECS` | Set the scan-cache staleness window (default 0, cache off) |
| `--keep`     | Keep the seeded files on exit and print where they landed  |

> [!NOTE]
> Marker buttons write real `.no_ebook` / `.ebook_elsewhere` files into the seeded tree. Pass `--keep` to inspect them after exit. Otherwise the temp directory is removed on shutdown.

For a live-reload loop while iterating on the UI, run `bacon explore` instead. It rebuilds and reruns the harness on a fixed port whenever `src/` or `assets/` change.

## Commit style

Conventional Commits: `type(scope): subject`. A `feat` or `fix` carries a body explaining the why and the effect, with a scope caveat where one applies (`No behavior change.`, `Prose only: ...`). A trivial change (a one-line doc edit, an ADR record) can be subject-only.

Work lands on `main` by rebase and fast-forward only, so each commit sits inline on a linear history with no merge commit above it to carry context. Keep commits granular and independent.

## Where work lives

- Issues, PRDs, and implementation plans live under `.scratch/<feature>/`, gitignored by default.
- ADRs live at `docs/adr/NNNN-kebab-title.md`. The template and amendment convention are in [`docs/adr/README.md`](docs/adr/README.md).

## PR expectations

Open an issue first for non-trivial work so the design conversation happens before code does. Keep commits granular. Each one reads on its own and passes CI on its own. Include the user-facing why in the commit body.

If you change anything that renders in the UI (HTML in `src/web.rs`, styles in `assets/app.css`, behavior in `assets/app.js`), include the UI harness command you used to eyeball it (e.g. `cargo run --example explore -- mixed-forest`).
