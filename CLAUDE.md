# missing-ebooks

Lightweight web server to surface audiobook folders that are missing their ebooks.

## Agent skills

### Issue tracker

Issues and PRDs live as markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default triage vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Verifying UI changes

After changing the rendered UI (HTML in `src/web.rs`, styles in `assets/app.css`, or behavior in `assets/app.js`), run the seeded UI harness, confirm it is serving, and hand the user a clickable localhost link so they can verify it visually:

    cargo run --example explore -- mixed-forest --port 8919

`examples/explore.rs` serves the production router against a synthetic library in a temp directory and tears it down on Ctrl-C. Pick the scenario that exercises the states your change touches: `mixed-forest` (flagship, three roots: two forests and a clean root), `messy-shelf` (inconsistent organization and mixed depth), `clean-error`, `root-flagged`, `pre-marked`, or `big-library` (volume and scroll). Point out what to look at, and stop the harness once the user is done.

Several worktrees can run this harness at once, so the port may already be taken by another agent. Check it is free with `lsof -iTCP:8919 -sTCP:LISTEN` before binding and pick another if it is not. To stop yours, match by working directory instead of killing every `explore`: each worktree has its own `target/`, so `lsof -a -p <pid> -d cwd` shows the owning worktree, and you only stop the instance whose cwd is yours. Never blanket-kill `explore` processes, or you take down another agent's harness.

## Committing and merging

Work lands on `main` by rebase and fast-forward only, so each commit sits inline on a linear history with no merge commit above it to carry context. Keep commits granular and don't squash (this is pre-release); each one has to read on its own.

Commits follow Conventional Commits (`type(scope): subject`). A `feat` or `fix` carries a body explaining the why and the effect, with a scope caveat where one applies (`No behavior change.`, `Prose only: ...`). A trivial change (a one-line doc edit, an ADR record) can be subject-only.
