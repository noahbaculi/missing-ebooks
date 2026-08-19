# Root config file locations

Question: can `deny.toml`, `taplo.toml`, `tsconfig.json`, `_typos.toml`, and `bacon.toml` move out of the repo root without breaking their tools?

Checked against local CLI help for the installed tool versions and this repo's current invocations in `mise.toml`, `.githooks/pre-commit`, `.github/workflows/ci.yml`, and `CONTRIBUTING.md`.

## Summary

| File | Can move? | Recommendation |
| --- | --- | --- |
| `deny.toml` | Yes, with `cargo deny check --config <path>` | Safe to move if every `cargo deny` invocation is updated |
| `taplo.toml` | Yes, with `taplo format --config <path>` or `TAPLO_CONFIG` | Safe to move, but it makes the common formatter command longer |
| `tsconfig.json` | Yes, with `tsc --project <path>` | Safe to move, but this repo's hook tests currently assume the root path |
| `_typos.toml` | Yes, with `typos --config <path>` | Safe to move, but root auto-discovery is the normal low-friction path |
| `bacon.toml` | Not cleanly as a file move | Keep at root unless we are willing to change how developers launch bacon |

## `deny.toml`

`cargo deny check --help` says `--config <CONFIG>` sets the config path and defaults to `<cwd>/deny.toml` when omitted. The top-level `cargo-deny --help` also has `--manifest-path <MANIFEST_PATH>`, so the manifest context and policy file location can be specified independently.

Current repo usage:

- `mise.toml` does not currently run `cargo deny` in `mise run lint`.
- `SECURITY.md` says the publish workflow re-runs `cargo deny check all`.

Recommendation: movable. If it moves to something like `tools/deny.toml` or `config/deny.toml`, update every workflow or release command to `cargo deny check --config tools/deny.toml all`. If the publish workflow has a plain `cargo deny check all`, moving the file without updating that command would break it.

## `taplo.toml`

`taplo format --help` says `--config <CONFIG>` sets the Taplo configuration path, and the same setting can come from `TAPLO_CONFIG`. It also supports `--no-auto-config`, which confirms config discovery is implicit unless overridden.

Current repo usage:

- `mise.toml` runs `taplo format --check`.
- `.githooks/pre-commit` runs `taplo format --check` when staged TOML files change.
- `taplo.toml` itself links Taplo's configuration docs.

Recommendation: movable, but I would only move it if root cleanliness beats command simplicity. Both `mise.toml` and `.githooks/pre-commit` would need `taplo format --config tools/taplo.toml --check` or an exported `TAPLO_CONFIG`. Keeping it at root preserves the familiar bare `taplo format` command.

## `tsconfig.json`

`tsc --help --all` says `--project, -p` compiles the project given a path to its configuration file or to a folder with a `tsconfig.json`.

Current repo usage:

- `mise.toml` runs `tsc --noEmit`.
- `.githooks/pre-commit` delegates JS checks to `mise run typecheck`.
- `tests/hooks/pre-commit.sh` stages `tsconfig.json` directly to verify that config changes trigger type checking.
- `CONTRIBUTING.md` explicitly names `tsconfig.json` as the check-only TypeScript config.

Recommendation: movable. Update `mise.toml` to `tsc --project tools/tsconfig.json --noEmit` or equivalent. Also update the pre-commit trigger regex, hook tests, and contributing text. Watch the relative paths inside `include` and `exclude`: when a config file moves, TypeScript resolves those paths relative to the config file, so either keep the config in root or rewrite entries like `../assets/app.js` if moved under a subdirectory.

## `_typos.toml`

`typos --help` says `--config <CUSTOM_CONFIG>` sets a custom config file and `--isolated` ignores implicit configuration files.

Current repo usage:

- `mise.toml` runs bare `typos`.
- `.githooks/pre-commit` runs bare `typos`.
- `_typos.toml` points to the upstream typos project.

Recommendation: movable. Update both invocations to `typos --config tools/_typos.toml`. I would not move it unless we also centralize all tool configs, because spelling checks are run from several contexts and the root file is the least surprising path.

## `bacon.toml`

`bacon --help` exposes `--project <project>`, which sets the project and working directory, and `--config-toml <CONFIG_TOML>`, which passes configuration as a TOML string. The help also says `--watch` can override paths computed from the project type and `bacon.toml` file. It does not show a `--config <path>` option for pointing at an arbitrary config file.

Current repo usage:

- `CONTRIBUTING.md` tells developers to run `bacon explore`.
- `mise.toml` comments explain that `bacon.toml` backs the UI loop.

Recommendation: keep at root. Moving it would likely mean losing the simple `bacon explore` command, replacing it with inline `--config-toml`, a wrapper task, or a different working directory trick. That is not worth it for root cleanup.

## Preferred cleanup shape

If we want a consistent rule, use this:

- Keep root config files that tools auto-discover and humans run directly: `bacon.toml`, `taplo.toml`, `_typos.toml`, probably `tsconfig.json` unless root becomes a hard goal.
- Move policy or less-human files only when the command already lives behind `mise`: `deny.toml` is the best candidate.
- If we choose a stronger root-minimal convention, put these in `tools/` and update every command to pass explicit config paths. That is feasible for all except `bacon.toml`, which should stay root.
