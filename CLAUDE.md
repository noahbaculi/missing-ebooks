# missing-ebooks

Lightweight web server to surface audiobook folders that are missing their ebooks.

## Agent skills

### Issue tracker

Issues and PRDs live locally under `.scratch/<feature>/`, gitignored. See `docs/agents/issue-tracker.md` for the layout convention.

### Triage labels

Default triage vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Superpowers artifacts

Files produced by superpowers skills (anything under `.superpowers/`, `docs/superpowers/`, `.serena/`, `.worktrees/`, or other agent scratch trees listed in `.gitignore`) do not get committed by default. A skill's own default path or wording (for example writing-plans suggesting `docs/superpowers/plans/`) is not authority to commit; only an explicit instruction from the user is. Absent that, leave the artifact in the gitignored path for the local agent session and do not stage it. If the artifact is one this repo treats as durable (a PRD, an implementation plan, an issue) and the user wants it tracked, route it into `.scratch/<feature>/` per `docs/agents/issue-tracker.md`. Do not edit `.gitignore` to track a superpowers path.

## Verifying UI changes

After changing the rendered UI (HTML in `src/web.rs`, styles in `assets/app.css`, or behavior in `assets/app.js`), run the seeded UI harness, confirm it is serving, and hand the user a clickable localhost link so they can verify it visually:

    cargo run --bin explore -- mixed-forest

`src/bin/explore.rs` serves the production router against a synthetic library in a temp directory and tears it down on Ctrl-C. Pick the scenario that exercises the states your change touches: `mixed-forest` (flagship, three roots: two forests and a clean root), `messy-shelf` (inconsistent organization and mixed depth), `clean-error`, `root-flagged`, `pre-marked`, or `big-library` (volume and scroll). Point out what to look at, and stop the harness once the user is done.

With no `--port`, the harness binds the production default 13379 (see ADR-0011) so the URL matches a real deployment, and prints the address it actually bound. Before launching, check whether 13379 is free with `lsof -i :13379`. If a sibling worktree's harness or a real instance holds it, pass the next port up explicitly (`--port 13380`, then `13381`, and so on until one is free) so the URL stays guessable. Do this rather than letting the harness fall back on its own: an unset `--port` lands on a random OS-assigned port the user has to read off the output and retype. The harness's own ADR-0011 fallback still runs as the safety net if you skip the check; this agent-side convention layers on top of it, not in place of it. Still read the printed URL to confirm what it bound. To stop yours, match by working directory instead of killing every `explore`: each worktree has its own `target/`, so `lsof -a -p <pid> -d cwd` shows the owning worktree, and you only stop the instance whose cwd is yours. During normal work, never blanket-kill `explore` processes, or you take down another agent's harness.

When the user explicitly asks for a fresh reset, that restraint is off: sweep every stray harness with `pkill -f 'target/debug/explore'` to catch instances that were orphaned and never cleaned up. Clear the visual-verification browsers in the same pass, since neither they nor the harness auto-clean. A Playwright session from the `playwright-cli` skill stays open until closed, so run `playwright-cli close-all` (or `playwright-cli kill-all` to force it); `playwright-cli list` shows what is still around.

## Agent toolbox

Run `mise tasks` to list every check. The shortcuts agents reach for most:

- `mise run check` runs the full pre-commit equivalent. Run before claiming
  work is done.
- `mise run test` runs nextest plus doctests.
- `mise run lint` runs fmt, clippy, typos, taplo, and the unused-dep check.
- `bacon` gives continuous fmt/clippy feedback while editing (see `bacon.toml`).
- `sg run -p '<pattern>' -l rust src/` does structural search across Rust
  source. Use for refactors where regex would over- or under-match.

## Committing and merging

Work lands on `main` by rebase and fast-forward only, so each commit sits inline on a linear history with no merge commit above it to carry context. Keep commits granular and don't squash (this is pre-release); each one has to read on its own.

Commits follow Conventional Commits (`type(scope): subject`). A `feat` or `fix` carries a body explaining the why and the effect, with a scope caveat where one applies (`No behavior change.`, `Prose only: ...`). A trivial change (a one-line doc edit, an ADR record) can be subject-only.

## Pre-commit hook

The committed `.githooks/pre-commit` runs fmt, clippy, `cargo doc -D warnings`, and (for asset/accent-test changes) `mise run test:accent`. `mise.toml` auto-activates it via its `[hooks] enter` entry the first time you cd in, so fresh worktrees do not need a manual `mise run setup`. The activation guard is idempotent. Never bypass with `--no-verify`.
