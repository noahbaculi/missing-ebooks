# Pre-commit hook auto-activates via mise's enter hook

`.githooks/pre-commit` is committed and runs `cargo fmt`, `cargo clippy`, `cargo doc -D warnings`, and `mise run test:accent` (each gated on staged paths), mirroring the CI jobs in `.github/workflows/ci.yml`. The hook only runs if `core.hooksPath` points at `.githooks`, which used to require a one-time `mise run setup` per clone. Contributors and agents working in fresh worktrees forgot, the hook stayed dormant, and CI absorbed the failures: eight `fmt` breaks and five `docs` breaks in the last twelve red runs would have been caught locally.

`mise.toml` now carries a `[hooks] enter` entry that sets `core.hooksPath=.githooks` the first time the directory becomes active under mise's shell integration. The entry is guarded so it short-circuits when the value is already correct, and it fires per worktree because each worktree has its own `.git/config`. `mise run setup` stays as the manual fallback for clones without mise shell integration loaded.

We considered three alternatives and set them aside. `cargo-husky` installs hooks on first `cargo test` but adds a dev-dependency and does not fire until tests run. A `build.rs` side effect would install on every build but pollutes a build script with non-build behavior and skips the docs-only commit path. A manual-only `mise run setup` with louder onboarding leaned on humans (and agents) reading instructions, which is exactly the failure mode the data showed.

Revisit if mise's shell integration stops being a reasonable baseline for the project's contributors, or if the hook grows expensive enough that the shell script needs framework features (parallelism, watcher, language selection) that a third-party tool would carry better than a hand-rolled script.
