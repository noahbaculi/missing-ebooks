# Library surface

Status: done

Audit cluster from `deep-dive/missing-ebooks-audit-2026-06-26.md`: issues 5, 6, 25, 28. The crate today exposes 16 `pub mod`s and ~58 `pub` items with no third-party consumer. Every `pub` exists so the demo bin, examples, benches, or integration tests can reach in. `scenarios.rs` and `synthetic.rs` (1360 LOC combined) ship in the lib API as a synthetic audiobook seeder. Several types are `pub` but only `pub(crate)`-reachable in practice.

## Goal

Treat the crate as binary-by-construction. Remove the accidental public-API obligation without restructuring the codebase.

## Non-goals

- Workspace split or extracting a `core` crate.
- Removing `lib.rs`. Cargo needs a crate root for integration tests, examples, benches, and the demo bin to share code.
- Reshaping `scenarios::touch`. All callsites in `src/` are inside `#[cfg(test)]` modules already.
- Any work outside this cluster (ADR amendments, `.scratch/` decision, publish hygiene beyond `publish = false`, etc.).

## Design

### Crate posture

`Cargo.toml`: `publish = false`. Closes the accidental-`cargo publish` risk that issue 14 also names.

`src/lib.rs`: add `#![doc(hidden)]` and a one-paragraph crate-level doc that names the lib as internal scaffolding for the binaries with no semver promise. Module declarations stay `pub` because cargo's integration-test and example boundary requires it.

### Fixture gating

New cargo feature `fixtures`, default-off. Gate the two modules at their `pub mod` line:

    #[cfg(any(test, feature = "fixtures"))]
    pub mod scenarios;
    #[cfg(any(test, feature = "fixtures"))]
    pub mod synthetic;

The `cfg(test)` arm keeps the in-crate test modules in `scanner`, `state`, `web`, `web/render`, `autosync`, `demo/handlers`, and `demo/overlay` compiling without the feature flag (those are the only `src/` callers of `scenarios::touch`, `find_scenario`, and `materialize`).

Consumers outside the crate's own test build declare the feature:

- `[[bin]] name = "missing-ebooks-demo"` gets `required-features = ["fixtures"]`. The demo's whole purpose is seeding a synthetic scenario.
- `[[example]] name = "explore"` gets `required-features = ["fixtures"]`.
- `[[example]] name = "tree_bench"` gets `required-features = ["fixtures"]`.
- `[[bench]] name = "render"` gets `required-features = ["fixtures"]`.

Any `tests/` integration test that reaches `scenarios` or `synthetic` needs `--features fixtures`. Whether one exists is checked during implementation. If yes, the pre-commit hook's `cargo test` and CI test job both gain `--features fixtures`.

### Demote accidental pub

- `src/demo/session.rs`: `SessionId(pub String)` becomes `SessionId(String)` with `pub(crate) fn new(s: String) -> Self` and `pub(crate) fn as_str(&self) -> &str`. Touch the one production read site to use `as_str`.
- `src/state.rs`: `RawViewStore`, `RawView`, `Applied`, `WriteError`, `WriteFailure` drop to `pub(crate)`.
- Sweep: any other top-level `pub` item that nothing under `tests/`, `examples/`, `benches/`, or `src/bin/` touches gets demoted to `pub(crate)`. The concrete list is built during implementation by grepping the four directories for each `pub` item in `src/`.

## Validation

- `cargo check` builds without `fixtures` and confirms the lib half does not require `scenarios` or `synthetic`.
- `cargo build --features fixtures` builds the demo bin, the fixture-gated example, and the fixture-gated bench.
- `cargo test --features fixtures` runs green.
- `cargo doc --no-deps -D warnings` confirms `#![doc(hidden)]` leaves no dead links.
- `cargo install --path . --root /tmp/mb-install --locked` produces only `missing-ebooks`. The same command with `--features fixtures` produces both `missing-ebooks` and `missing-ebooks-demo`.

## Risks

- A `tests/` integration test reaching `scenarios` or `synthetic` would need `--features fixtures` on every `cargo test` invocation, including the pre-commit hook. The implementation pass checks this before committing to a hook change.
- `Arc::make_mut`-style reachability in `state.rs` is unaffected by the visibility demotions; behavior does not change.
- `publish = false` blocks `cargo publish`. Anyone wanting a one-off publish has to flip it deliberately, which is the intent.

## Out of scope, deferred to other clusters

- Issue 14 (`keywords`, `categories`, `exclude` list) is moot under `publish = false`.
- Issues 11, 12, 30 (autosync and SSE work in critical sections) are a separate cluster.
- Issues 8, 9, 10 (ADR amendments, `.scratch/` curate-or-ignore, `.claude/` ignore) are the next cluster.
