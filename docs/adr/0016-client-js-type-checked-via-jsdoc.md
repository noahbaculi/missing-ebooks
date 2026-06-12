# Client JS is type-checked via JSDoc, with no build step

`assets/app.js` is plain JavaScript, embedded with `include_str!` and served
verbatim. A check-only TypeScript pass reads it: a `// @ts-check` pragma, JSDoc
annotations, and a `tsconfig.json` with `checkJs` and `noEmit` under `strict`.
The htmx surface and the app's own custom events are typed by a hand-written
ambient stub at `types/htmx.d.ts`. There is no `package.json`, no lockfile, no
`node_modules`, and nothing is emitted: the source stays the shipped artifact.
The check is pinned through mise and runs in the pre-commit hook and a CI job,
beside `fmt` and `clippy`.

We considered a full TypeScript migration, compiling `app.ts` to `app.js`. We
set it aside. It would put a Node build step in front of `include_str!`, which
then embeds a generated file with a new "is the artifact in sync?" failure mode,
force Node into the multiarch Docker build (ADR-0014), and need sourcemaps to
debug. That cost only pays off for a module-split frontend, which is a departure
from the deliberate "htmx plus one small vanilla file" shape of the client.

We chose a hand-written htmx stub over `@types/htmx.org` because the published
types need an npm install, are community-maintained, and the code uses only a
small, stable corner of the API. The stub ships nothing and documents exactly
which htmx surface the app depends on.

If the client ever grows into multiple modules that need bundling, revisit this:
at that point a real build step would carry its weight, and full TypeScript with
emitted output would concentrate genuine value rather than add a compile step in
front of a file that ships as-is.
