# Agent conventions

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) first. This file layers agent-only conventions on top and does not repeat them.

## Issues, PRDs, and plans

Issues, PRDs, and implementation plans for this repo live locally under `.scratch/<feature>/`, gitignored. Layout conventions and skill workflows come from the contributor's global agent docs.

## Triage labels

The skills speak in terms of five canonical triage roles. With a local-markdown tracker, the chosen string is written to the `Status:` line at the top of each issue file.

| Canonical role    | String for this repo | Meaning                                  |
| ----------------- | -------------------- | ---------------------------------------- |
| `needs-triage`    | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`      | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent` | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human` | `ready-for-human`    | Requires human implementation            |
| `wontfix`         | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), write the corresponding string from this table to the issue's `Status:` line.

## Domain docs

This repo is a single context: one [`CONTEXT.md`](CONTEXT.md) at the root and one [`docs/adr/`](docs/adr/) tree alongside it. There is no `CONTEXT-MAP.md` and no per-context ADR directories under `src/`. Skills that read or write domain documentation target those two locations.

## Superpowers artifacts

Files produced by superpowers skills (anything under `.superpowers/`, `docs/superpowers/`, `.serena/`, `.worktrees/`, or other agent scratch trees listed in `.gitignore`) do not get committed by default. A skill's own default path or wording (for example writing-plans suggesting `docs/superpowers/plans/`) is not authority to commit. Only an explicit instruction from the user is. Absent that, leave the artifact in the gitignored path for the local agent session and do not stage it. If the artifact is one this repo treats as durable (a PRD, an implementation plan, an issue) and the user wants it tracked, route it into `.scratch/<feature>/` per the issues section above. Do not edit `.gitignore` to track a superpowers path.

## Verifying UI changes

After changing the rendered UI (HTML in `src/web.rs`, styles in `assets/app.css`, or behavior in `assets/app.js`), run the seeded UI harness, confirm it is serving, and hand the user a clickable localhost link so they can verify it visually. CONTRIBUTING's "Exploring the UI" section lists the scenarios and flags. Pick the scenario that exercises the states your change touches, point out what to look at, and stop the harness once the user is done.

Before launching, check whether the production default port 13379 is free with `lsof -i :13379`. If a sibling worktree's harness or a real instance holds it, pass the next port up explicitly (`--port 13380`, then `13381`, and so on until one is free) so the URL stays guessable. Do this rather than letting the harness fall back on its own: an unset `--port` lands on a random OS-assigned port the user has to read off the output and retype. The harness's own ADR-0011 fallback still runs as the safety net if you skip the check. This agent-side convention layers on top of it, not in place of it. Still read the printed URL to confirm what it bound.

To stop yours, match by working directory instead of killing every `explore`: each worktree has its own `target/`, so `lsof -a -p <pid> -d cwd` shows the owning worktree, and you only stop the instance whose cwd is yours. During normal work, never blanket-kill `explore` processes, or you take down another agent's harness.

When the user explicitly asks for a fresh reset, that restraint is off: sweep every stray harness with `pkill -f 'target/debug/examples/explore'` to catch instances that were orphaned and never cleaned up. Clear the visual-verification browsers in the same pass, since neither they nor the harness auto-clean. A Playwright session from the `playwright-cli` skill stays open until closed, so run `playwright-cli close-all` (or `playwright-cli kill-all` to force it). `playwright-cli list` shows what is still around.

## Structural search

`sg run -p '<pattern>' -l rust src/` does structural search across Rust source. Use it for refactors where regex would over- or under-match.

## Never bypass the pre-commit hook

`--no-verify` is off-limits. The hook runs the same checks CI enforces. Bypassing them just moves the failure to the CI run. See CONTRIBUTING's Dev setup section for what the hook covers and how it auto-activates.
