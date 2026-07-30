# Contributing

Thanks for the interest. This repo is a single-author hobby project, so the contribution surface is small: open an issue first for anything non-trivial, and keep changes scoped and well-explained.

## Dev setup

`mise install` provisions the pinned tools (cargo-bacon, cargo-deny, node, typescript, taplo, typos, cargo-machete).

The committed `.githooks/pre-commit` runs `cargo fmt`, `cargo clippy`, `cargo doc -D warnings`, and (for asset or accent-test changes) `mise run test:accent`. `mise.toml`'s `[hooks] enter` entry auto-activates the hook on the first `cd` into the worktree (see [ADR-0026](docs/adr/0026-pre-commit-hook-auto-activation.md)). Contributors who do not use mise shell integration run `mise run setup` once per clone (and once per worktree) to point git at the same hooks.

Never bypass the hook with `--no-verify`. The hook runs the same checks CI enforces; bypassing them just moves the failure to the CI run.

## Build and test

```shell
cargo build --release
cargo test
cargo run --example explore -- mixed-forest    # UI harness on http://127.0.0.1:13379
```

The UI harness seeds a synthetic library into a temp directory and serves the production router against it. Scenarios: `mixed-forest`, `messy-shelf`, `clean-error`, `root-flagged`, `pre-marked`, `big-library`. Pass `--port N` to pin a port (the default 13379 falls back to an OS-assigned port if in use), `--ttl SECS` to set the cache window, or `--keep` to preserve the seeded files on exit.

MSRV is Rust 1.96 (matches `rust-toolchain.toml`).

`mise tasks` lists every check. The shortcuts most reached for:

- `mise run check` runs the full pre-commit equivalent. Run before claiming work is done.
- `mise run test` runs nextest plus doctests.
- `mise run lint` runs fmt, clippy, typos, taplo, and the unused-dep check.
- `bacon` gives continuous fmt/clippy feedback while editing (see `bacon.toml`).

## Commit style

Conventional Commits: `type(scope): subject`. A `feat` or `fix` carries a body explaining the why and the effect, with a scope caveat where one applies (`No behavior change.`, `Prose only: ...`). A trivial change (a one-line doc edit, an ADR record) can be subject-only.

Work lands on `main` by rebase and fast-forward only, so each commit sits inline on a linear history with no merge commit above it to carry context. Keep commits granular and don't squash (this is pre-release); each one has to read on its own.

## Where work lives

- Issues, PRDs, and implementation plans live under `.scratch/<feature>/`, gitignored. Layout convention is in [`docs/agents/issue-tracker.md`](docs/agents/issue-tracker.md).
- ADRs live at `docs/adr/NNNN-kebab-title.md`. The template and amendment convention are in [`docs/adr/README.md`](docs/adr/README.md).
- The domain glossary is at [`CONTEXT.md`](CONTEXT.md) in the repo root.
- The triage label vocabulary is at [`docs/agents/triage-labels.md`](docs/agents/triage-labels.md). The five canonical roles map to strings written into each issue's `Status:` line.

## PR expectations

Open an issue first for non-trivial work so the design conversation happens before code does. Keep commits granular. Each one reads on its own and passes CI on its own. Include the user-facing why in the commit body, along with the scope caveat (`No behavior change.`, `Prose only:`, etc.) when one applies.

If you change anything that renders in the UI (HTML in `src/web.rs`, styles in `assets/app.css`, behavior in `assets/app.js`), include the UI harness command you used to eyeball it (e.g. `cargo run --example explore -- mixed-forest`).
