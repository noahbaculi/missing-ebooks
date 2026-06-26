# Release Blockers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the four release-critical audit items: demo session mutex poison recovery, bounded demo render cost via a set-backed `MarkOverlay`, demo binary rename + `fixtures` feature placeholder, and working `--help`/`--version` on both binaries. Demo `/unmark` route hitches a ride on the storage rework.

**Architecture:** Five sequential commits, each leaving `cargo test` green. The production server (`src/state.rs`, `src/web/`, `src/scanner.rs`, `src/autosync.rs`, `src/raw_view.rs`) is untouched. Demo storage moves from `Vec<Mark>` to `HashSet<MarkKey>`; per-request render moves from clone-and-replay (`O((M+1) × F)`) to a borrowing overlay walk (`O(F × depth)`). CLI gains clap-derived `Cli` structs that layer on top of the existing env-var precedence in `Config::load` and `DemoConfig`.

**Tech Stack:** Rust 1.96 (per `rust-toolchain.toml`), axum 0.8, tokio 1.52, maud 0.27, clap 4 (added by Task 2), `tower::ServiceExt::oneshot` for handler tests, `http_body_util::BodyExt` for body collection. No new runtime deps beyond clap.

**Source:** `.scratch/release-blockers/spec.md`.

## Global Constraints

These apply to every task. The task body does not restate them.

- **Conventional Commits.** Subject: `type(scope): subject`. `feat`/`fix` carry a body explaining the why and a scope caveat (`No behavior change.`, `Prose only: ...`) where one applies. Trivial chores can be subject-only. Source: `CLAUDE.md` → "Committing and merging".
- **Pre-commit hook is mandatory.** `.githooks/pre-commit` runs `cargo fmt`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo doc -D warnings`, and (on asset/accent changes) `mise run test:accent`. Never bypass with `--no-verify`. The hook activates automatically on `cd` via the `[hooks] enter` entry in `mise.toml`.
- **Every commit compiles and tests pass.** If a commit needs to leave a function temporarily unused, `#[allow(dead_code)]` it with a comment naming the commit that consumes it. The plan calls these out explicitly.
- **Linear history; no merge commits.** Work lands by rebase-and-fast-forward only.
- **No em dashes** in prose, docstrings, or commit messages (per `AGENTS.md`). Use a comma, semicolon, period, or parens instead.
- **Production server logic is untouched.** Edits outside `src/main.rs`, `src/bin/demo.rs`, `src/demo/`, `Cargo.toml`, and tests are out of scope. `src/state.rs`, `src/web/`, `src/scanner.rs`, `src/autosync.rs`, `src/raw_view.rs` are read but not written.
- **Markdown is unwrapped** (one paragraph per line, soft-wrap in the editor). Per `AGENTS.md`.
- **Rust toolchain:** 1.96 (pinned in `rust-toolchain.toml`). Edition 2024.
- **MSRV note on clap.** clap 4.x supports Rust 1.74+; well under our 1.96 floor.

## Spec-vs-reality reconciliations

Four places where the spec sketches assumed a code shape that does not exist today. Implement the corrected version in this plan and call it out in the relevant commit body.

1. **`Config::to_toml_string` does not exist.** Today's `--print-config` at `src/main.rs:124-127` prints the static `CONFIG_TEMPLATE` constant (`src/config.rs:238`). The spec's code sketch on line 286 (`config.to_toml_string()`) is aspirational. The spec's prose on line 291 says "preserves today's behavior exactly", which means: print `CONFIG_TEMPLATE`. Task 2 keeps the template path.
2. **No `src/demo/mod.rs`.** The demo module is declared at `src/demo.rs` (single-file form) with `pub mod banner; pub mod handlers; pub mod session; pub mod state;`. Task 5 adds `pub mod overlay;` to `src/demo.rs`, not a new `mod.rs`.
3. **`Marker::ALL` already exists** at `src/marker.rs:19` as `pub const ALL: [Marker; 2] = [Marker::NoEbook, Marker::EbookElsewhere]`. The spec's "if it doesn't exist yet, add it" note is moot. No edit to `src/marker.rs`.
4. **`MarkRequest` is `pub(crate)`** with `pub(crate)` fields (`src/web.rs:40-49`). Demo handlers reach it via `use crate::web::MarkRequest`. No visibility change is needed; tests live inside `src/demo/handlers.rs::tests` where `pub(crate)` is reachable.

## File map

Touched per task. Exact paths only.

**Task 1 (binary rename + feature placeholder)**
- Modify: `Cargo.toml`

**Task 2 (clap CLI on both binaries)**
- Modify: `Cargo.toml`, `src/main.rs`, `src/bin/demo.rs`

**Task 3 (demo session mutex poison recovery)**
- Modify: `src/demo/state.rs`, `src/demo/handlers.rs`
- Test: `src/demo/state.rs::tests`

**Task 4 (set-based mark storage + folder-existence validation at `/mark`)**
- Modify: `src/demo/session.rs`, `src/demo/handlers.rs`
- Test: `src/demo/session.rs::tests`, `src/demo/handlers.rs::tests`

**Task 5 (MarkOverlay rendering, delete `derive_view`, add `/unmark` route)**
- Create: `src/demo/overlay.rs`
- Modify: `src/demo.rs` (add `pub mod overlay;`), `src/demo/handlers.rs`
- Test: `src/demo/overlay.rs::tests`, `src/demo/handlers.rs::tests`

---

## Task 1: Rename demo binary and declare the `fixtures` feature placeholder

**Goal of this commit:** Audit item #3. After this commit, `cargo install missing-ebooks` no longer plants a binary called `demo` on the user's PATH.

**Files:**
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - A `fixtures` Cargo feature (defined empty here, gated bodies land in spec B).
  - A `missing-ebooks-demo` binary that requires `--features fixtures`. The default `cargo install missing-ebooks` produces only the `missing-ebooks` binary.

**Background.** Today `Cargo.toml` declares neither a `[features]` table nor any `[[bin]]` block. The production binary at `src/main.rs` and the demo binary at `src/bin/demo.rs` are auto-discovered by cargo's default binary discovery; the demo's binary name is `demo` (derived from the filename). To rename and to require a feature, we add explicit `[[bin]]` entries for both. Once any `[[bin]]` is declared, auto-discovery still applies to *other* files, but the explicit entry wins for the path it names.

- [ ] **Step 1: Verify the current install plants `demo` and `missing-ebooks`**

  Run from the repo root:

  ```bash
  cargo install --path . --root /tmp/mb-install-before --locked
  ls /tmp/mb-install-before/bin/
  ```

  Expected: output includes both `demo` and `missing-ebooks`. This is the audit's reproducer; capturing it now makes the after-state self-evident.

- [ ] **Step 2: Edit `Cargo.toml`**

  Add the `[features]` table and the two `[[bin]]` blocks. Place the `[features]` table immediately after the existing `[package]` block (before `[[example]]`). Place the `[[bin]]` blocks together after the `[[example]]` block group.

  Insert after `[package]`:

  ```toml
  [features]
  # Defined empty here so the demo binary can require it; spec B will gate
  # `scenarios.rs` and `synthetic.rs` under the same feature. The empty body
  # means no current behavior changes: the demo still builds today exactly
  # when `--features fixtures` is passed.
  fixtures = []
  ```

  Insert after the `[[example]]` blocks (before `[[bench]]`):

  ```toml
  [[bin]]
  name = "missing-ebooks"
  path = "src/main.rs"

  # Renamed from the auto-discovered `demo` so `cargo install missing-ebooks`
  # does not plant a generic-named binary on user PATHs. Gated behind the
  # `fixtures` feature so the default install omits it entirely.
  [[bin]]
  name = "missing-ebooks-demo"
  path = "src/bin/demo.rs"
  required-features = ["fixtures"]
  ```

- [ ] **Step 3: Verify the production binary still builds without the feature**

  ```bash
  cargo build --locked --bin missing-ebooks
  ```

  Expected: builds clean. `src/bin/demo.rs` is not compiled because the bin is gated.

- [ ] **Step 4: Verify the demo binary builds with the feature**

  ```bash
  cargo build --locked --features fixtures --bin missing-ebooks-demo
  ```

  Expected: builds clean. The output binary is at `target/debug/missing-ebooks-demo`.

- [ ] **Step 5: Verify the install plants only `missing-ebooks` by default**

  ```bash
  cargo install --path . --root /tmp/mb-install-after --locked
  ls /tmp/mb-install-after/bin/
  ```

  Expected: output is only `missing-ebooks`. No `demo`. No `missing-ebooks-demo`.

  Then with the feature:

  ```bash
  cargo install --path . --root /tmp/mb-install-after-fixtures --locked --features fixtures
  ls /tmp/mb-install-after-fixtures/bin/
  ```

  Expected: both `missing-ebooks` and `missing-ebooks-demo`.

- [ ] **Step 6: Run the full test suite**

  ```bash
  cargo test --locked
  cargo test --locked --features fixtures
  ```

  Expected: both pass. No test depends on the binary name; the existing demo handler tests link against `missing_ebooks::demo::*` through the library, not through any binary.

- [ ] **Step 7: Commit**

  ```bash
  git add Cargo.toml
  git commit -m "chore(cargo): rename demo binary to missing-ebooks-demo, declare fixtures feature

  cargo install missing-ebooks no longer plants a generic 'demo' binary on
  user PATHs. The renamed binary is gated behind a new 'fixtures' feature so
  the default install omits it entirely. The feature body is empty in this
  commit; spec B will gate scenarios.rs and synthetic.rs under the same flag.

  Audit item #3. No behavior change for the production binary."
  ```

---

## Task 2: Add `--help`, `--version`, `--print-config`, `--config` via clap

**Goal of this commit:** Audit item #4. `missing-ebooks --help`, `missing-ebooks --version`, `missing-ebooks-demo --help`, and `missing-ebooks-demo --version` all exit 0 with usage/version. `--print-config` and `--config` continue to work on production. The demo gains `--scenario` and `--bind` flags that override the corresponding env vars.

**Files:**
- Modify: `Cargo.toml`, `src/main.rs`, `src/bin/demo.rs`

**Interfaces:**
- Consumes: Task 1's binary structure.
- Produces:
  - `Cli` struct in `src/main.rs` with `print_config: bool` and `config: Option<PathBuf>`.
  - `Cli` struct in `src/bin/demo.rs` with `scenario: Option<String>` and `bind: Option<String>`.
  - Behavior preserved: `Config::load(cli.config.as_deref())` still reads env vars internally; `--print-config` still prints `CONFIG_TEMPLATE`.

**Background.** Today `src/main.rs:120-127` reads `--print-config` from a manual `std::env::args` vector before calling `Config::load`. `src/main.rs:134-144` hand-parses `--config`. Replacing both with `clap::Parser` gives `--help` and `--version` for free and produces friendlier error messages on bad invocations. The demo binary today has no arg parsing; `--help` is silently ignored and the server starts.

`Config::load` already takes `Option<&Path>` (`src/config.rs:131-142`). Env-var precedence (`MISSING_EBOOKS_*`) is internal to `Config::load` and unchanged. The CLI is a thin wrapper on top.

- [ ] **Step 1: Add the clap dependency to `Cargo.toml`**

  In the `[dependencies]` block, add (keep the block alphabetized; clap sorts between `axum` and `enum-map`):

  ```toml
  clap = { version = "4", features = ["derive"] }
  ```

  Verify it resolves:

  ```bash
  cargo build --locked --bin missing-ebooks
  ```

  Expected: builds clean.

- [ ] **Step 2: Write the failing test for production CLI help/version**

  At the bottom of `src/main.rs`, replace the existing `#[cfg(test)] mod tests { ... }` block with the version below. The existing `parses_config_path_in_both_forms` test is dropped (the function it covers, `parse_config_path`, is deleted in Step 4).

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use clap::CommandFactory;
      use clap::Parser;

      #[test]
      fn help_flag_displays_help() {
          let err = Cli::try_parse_from(["missing-ebooks", "--help"]).unwrap_err();
          assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
      }

      #[test]
      fn version_flag_displays_version() {
          let err = Cli::try_parse_from(["missing-ebooks", "--version"]).unwrap_err();
          assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
      }

      #[test]
      fn print_config_is_a_bool_flag() {
          let cli = Cli::try_parse_from(["missing-ebooks", "--print-config"]).unwrap();
          assert!(cli.print_config);
          assert!(cli.config.is_none());
      }

      #[test]
      fn config_path_accepts_both_forms() {
          let cli = Cli::try_parse_from(["missing-ebooks", "--config", "/a/b.toml"]).unwrap();
          assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/a/b.toml")));
          let cli = Cli::try_parse_from(["missing-ebooks", "--config=/c/d.toml"]).unwrap();
          assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/c/d.toml")));
      }

      #[test]
      fn after_help_lists_env_vars() {
          // Pins the env-var enumeration so a future change that adds a var here
          // does not silently leave the after_help out of date.
          let help = Cli::command().render_help().to_string();
          for var in [
              "MISSING_EBOOKS_LIBRARY_ROOTS",
              "MISSING_EBOOKS_BIND",
              "MISSING_EBOOKS_PORT",
              "MISSING_EBOOKS_CONFIG",
              "MISSING_EBOOKS_LOG",
          ] {
              assert!(help.contains(var), "{var} missing from --help");
          }
      }
  }
  ```

- [ ] **Step 3: Run the test to verify it fails**

  ```bash
  cargo test --locked --bin missing-ebooks -- tests::
  ```

  Expected: FAIL with "cannot find struct, variant or union type `Cli` in this scope" (or similar). The `Cli` struct is defined in Step 4.

- [ ] **Step 4: Add the `Cli` struct and wire it into `main`**

  Replace the full contents of `src/main.rs` with:

  ```rust
  //! Server entry point: load config, build the shared state, and serve the
  //! read-only web UI. `--print-config` still emits the template and exits.

  use std::net::{IpAddr, SocketAddr};
  use std::path::PathBuf;
  use std::process::ExitCode;
  use std::sync::Arc;

  use clap::Parser;

  use missing_ebooks::config::{CONFIG_TEMPLATE, Config, ConfigError};
  use missing_ebooks::scanner::ScanSettings;
  use missing_ebooks::state::AppState;
  use missing_ebooks::web;

  /// Command-line surface. Environment variables remain the primary
  /// configuration path. Flags layer on top per `Config::load`'s precedence.
  #[derive(Parser, Debug)]
  #[command(
      name = "missing-ebooks",
      version,
      about = "Surface audiobook folders that hold audio but no matching ebook.",
      after_help = "Environment variables:\n  \
          MISSING_EBOOKS_LIBRARY_ROOTS  Colon-separated paths to scan.\n  \
          MISSING_EBOOKS_BIND           IP to bind, e.g. 127.0.0.1.\n  \
          MISSING_EBOOKS_PORT           TCP port, e.g. 8080.\n  \
          MISSING_EBOOKS_CONFIG         Optional config file path (same as --config).\n  \
          MISSING_EBOOKS_LOG            Tracing filter, e.g. info,missing_ebooks=debug.\n\
          \nSee README for the full env-var list."
  )]
  struct Cli {
      /// Print the bundled configuration template as TOML and exit.
      #[arg(long)]
      print_config: bool,

      /// Path to a configuration file. Defaults to MISSING_EBOOKS_CONFIG or none.
      #[arg(long, value_name = "PATH", env = "MISSING_EBOOKS_CONFIG")]
      config: Option<PathBuf>,
  }

  #[tokio::main]
  async fn main() -> ExitCode {
      missing_ebooks::telemetry::init();

      let cli = Cli::parse();

      if cli.print_config {
          print!("{CONFIG_TEMPLATE}");
          return ExitCode::SUCCESS;
      }

      let config = match Config::load(cli.config.as_deref()) {
          Ok(cfg) => cfg,
          Err(err @ ConfigError::MissingLibraryRoots) => {
              eprintln!("{err}");
              return ExitCode::from(2);
          }
          Err(err) => {
              eprintln!("error: {err}");
              return ExitCode::from(1);
          }
      };

      let settings = match ScanSettings::compile(config.scan_inputs()) {
          Ok(settings) => settings,
          Err(err) => {
              tracing::error!(error = %err, "invalid scan settings");
              return ExitCode::from(1);
          }
      };

      let ip: IpAddr = match config.bind.parse() {
          Ok(ip) => ip,
          Err(_) => {
              tracing::error!(bind = %config.bind, "bind is not a valid IP address");
              return ExitCode::from(1);
          }
      };
      if !ip.is_loopback() {
          tracing::warn!(
              bind = %config.bind,
              "binding to a non-loopback address; the server has no authentication"
          );
      }
      let addr = SocketAddr::new(ip, config.port);

      // Size the scan thread pool by the configured concurrency, not the core
      // count: the directory walk is bound by network round-trip latency, so the
      // threads mostly wait on the wire and stay useful well above the CPU count
      // (and survive a container CPU limit). build_global is called once per
      // process; a failure leaves rayon's default pool in place.
      if let Err(err) = rayon::ThreadPoolBuilder::new()
          .num_threads(config.scan_concurrency.max(1))
          .build_global()
      {
          tracing::warn!(error = %err, "could not size the scan thread pool; using rayon defaults");
      }

      let state = Arc::new(AppState::new(config, settings));
      let app = web::router(Arc::clone(&state));

      let listener = match tokio::net::TcpListener::bind(addr).await {
          Ok(listener) => listener,
          Err(err) => {
              tracing::error!(%addr, error = %err, "could not bind the listener");
              return ExitCode::from(1);
          }
      };
      tracing::info!(url = %format!("http://{addr}"), "missing-ebooks listening");

      // Warm the default (gaps-only) view in the background so the first viewer
      // after a restart does not pay the cold scan, which is slow over a network
      // mount. The server starts serving immediately; a request that arrives
      // before the warm finishes single-flights on the same cache lock, so this
      // never double-scans. The show-all slot stays lazy until first asked.
      tokio::spawn({
          let state = Arc::clone(&state);
          async move {
              // Warm the gaps-mode slot. The packaging is cheap; the cache
              // slot side effect is what we want.
              state.warm().await;
              tracing::debug!("startup cache warm complete");
          }
      });

      let serve =
          axum::serve(listener, app).with_graceful_shutdown(missing_ebooks::shutdown::signal());
      if let Err(err) = serve.await {
          tracing::error!(error = %err, "server error");
          return ExitCode::from(1);
      }
      ExitCode::SUCCESS
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use clap::CommandFactory;

      #[test]
      fn help_flag_displays_help() {
          let err = Cli::try_parse_from(["missing-ebooks", "--help"]).unwrap_err();
          assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
      }

      #[test]
      fn version_flag_displays_version() {
          let err = Cli::try_parse_from(["missing-ebooks", "--version"]).unwrap_err();
          assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
      }

      #[test]
      fn print_config_is_a_bool_flag() {
          let cli = Cli::try_parse_from(["missing-ebooks", "--print-config"]).unwrap();
          assert!(cli.print_config);
          assert!(cli.config.is_none());
      }

      #[test]
      fn config_path_accepts_both_forms() {
          let cli = Cli::try_parse_from(["missing-ebooks", "--config", "/a/b.toml"]).unwrap();
          assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/a/b.toml")));
          let cli = Cli::try_parse_from(["missing-ebooks", "--config=/c/d.toml"]).unwrap();
          assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/c/d.toml")));
      }

      #[test]
      fn after_help_lists_env_vars() {
          let help = Cli::command().render_help().to_string();
          for var in [
              "MISSING_EBOOKS_LIBRARY_ROOTS",
              "MISSING_EBOOKS_BIND",
              "MISSING_EBOOKS_PORT",
              "MISSING_EBOOKS_CONFIG",
              "MISSING_EBOOKS_LOG",
          ] {
              assert!(help.contains(var), "{var} missing from --help");
          }
      }
  }
  ```

  Note on `env = "MISSING_EBOOKS_CONFIG"`: clap reads the env var directly when no `--config` flag is passed. This subsumes the hand-rolled `MISSING_EBOOKS_CONFIG` resolution (which `Config::load` would otherwise pick up). Net behavior: identical precedence, single source of resolution.

- [ ] **Step 5: Run the production-bin tests and verify they pass**

  ```bash
  cargo test --locked --bin missing-ebooks -- tests::
  ```

  Expected: 5 tests pass.

- [ ] **Step 6: Smoke-test `--help` and `--version` end to end**

  ```bash
  cargo run --locked --bin missing-ebooks -- --help
  echo "exit: $?"
  cargo run --locked --bin missing-ebooks -- --version
  echo "exit: $?"
  ```

  Expected: both print sensible output and exit 0. The `--help` body includes the env-var block.

- [ ] **Step 7: Write the failing test for demo CLI**

  Add a `#[cfg(test)] mod tests` block at the bottom of `src/bin/demo.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use clap::CommandFactory;
      use clap::Parser;

      #[test]
      fn demo_help_flag_displays_help() {
          let err = Cli::try_parse_from(["missing-ebooks-demo", "--help"]).unwrap_err();
          assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
      }

      #[test]
      fn demo_version_flag_displays_version() {
          let err = Cli::try_parse_from(["missing-ebooks-demo", "--version"]).unwrap_err();
          assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
      }

      #[test]
      fn demo_scenario_and_bind_are_optional() {
          let cli = Cli::try_parse_from(["missing-ebooks-demo"]).unwrap();
          assert!(cli.scenario.is_none());
          assert!(cli.bind.is_none());
      }

      #[test]
      fn demo_flags_parse() {
          let cli = Cli::try_parse_from([
              "missing-ebooks-demo",
              "--scenario", "mixed-forest",
              "--bind", "0.0.0.0:9000",
          ]).unwrap();
          assert_eq!(cli.scenario.as_deref(), Some("mixed-forest"));
          assert_eq!(cli.bind.as_deref(), Some("0.0.0.0:9000"));
      }

      #[test]
      fn demo_after_help_lists_env_vars() {
          let help = Cli::command().render_help().to_string();
          for var in [
              "DEMO_BIND",
              "DEMO_SCENARIO",
              "DEMO_MAX_SESSIONS",
              "DEMO_IDLE_SECS",
              "DEMO_COOKIE_NAME",
          ] {
              assert!(help.contains(var), "{var} missing from demo --help");
          }
      }
  }
  ```

- [ ] **Step 8: Run the demo-bin tests to verify they fail**

  ```bash
  cargo test --locked --features fixtures --bin missing-ebooks-demo -- tests::
  ```

  Expected: FAIL: `Cli` is not defined.

- [ ] **Step 9: Add the `Cli` struct and wire it into demo `main`**

  Replace the full contents of `src/bin/demo.rs` with:

  ```rust
  //! The public demo server: one process, in-memory per-session marks.
  //!
  //! Seeds a synthetic library into a temp directory, scans it into shared base
  //! views, and serves the production UI with a demo banner. Each visitor is pinned
  //! to an in-memory session by a cookie. Their marks are replayed on top of the
  //! base view per request and never touch disk.

  use std::sync::Arc;
  use std::time::{Duration, Instant};

  use clap::Parser;

  use missing_ebooks::config::Config;
  use missing_ebooks::demo::handlers::router as demo_router;
  use missing_ebooks::demo::state::{DemoConfig, DemoState, build_state};
  use missing_ebooks::scanner::ScanSettings;
  use missing_ebooks::scenarios;

  /// Demo CLI surface. Flags override the matching env vars. Everything else
  /// continues to come from the environment (DEMO_MAX_SESSIONS, DEMO_IDLE_SECS,
  /// DEMO_COOKIE_NAME).
  #[derive(Parser, Debug)]
  #[command(
      name = "missing-ebooks-demo",
      version,
      about = "Run the public-facing demo with a synthetic library.",
      after_help = "Environment variables:\n  \
          DEMO_BIND          IP:port to bind, e.g. 127.0.0.1:8080.\n  \
          DEMO_SCENARIO      Seeded scenario name, e.g. mixed-forest.\n  \
          DEMO_MAX_SESSIONS  Hard cap on concurrent sessions.\n  \
          DEMO_IDLE_SECS     Session idle window before the reaper drops it.\n  \
          DEMO_COOKIE_NAME   Session cookie name.\n\
          \nScenarios: mixed-forest, messy-shelf, clean-error, root-flagged, \
          pre-marked, big-library."
  )]
  struct Cli {
      /// Scenario name. Overrides DEMO_SCENARIO.
      #[arg(long)]
      scenario: Option<String>,
      /// Bind address (IP:port). Overrides DEMO_BIND.
      #[arg(long)]
      bind: Option<String>,
  }

  /// Read one variable, falling back to `default` when it is unset or empty.
  fn var_or(name: &str, default: &str) -> String {
      match std::env::var(name) {
          Ok(value) if !value.trim().is_empty() => value,
          _ => default.to_string(),
      }
  }

  /// Build the demo config: defaults, then env-var overrides, then CLI overrides.
  /// CLI flags win because they sit closest to the invocation.
  fn load_config(cli: &Cli) -> anyhow::Result<DemoConfig> {
      let bind = cli
          .bind
          .clone()
          .unwrap_or_else(|| var_or("DEMO_BIND", "127.0.0.1:8080"));
      let scenario = cli
          .scenario
          .clone()
          .unwrap_or_else(|| var_or("DEMO_SCENARIO", "mixed-forest"));
      Ok(DemoConfig {
          bind,
          scenario,
          max_sessions: var_or("DEMO_MAX_SESSIONS", "1000").parse()?,
          idle: Duration::from_secs(var_or("DEMO_IDLE_SECS", "1200").parse()?),
          cookie_name: var_or("DEMO_COOKIE_NAME", "me_demo_sid"),
      })
  }

  /// Sweep idle sessions on a fixed tick.
  async fn run_reaper(state: Arc<DemoState>) {
      let mut tick = tokio::time::interval(Duration::from_secs(60));
      loop {
          tick.tick().await;
          let reaped = state.reap_idle(Instant::now());
          if reaped > 0 {
              tracing::info!(reaped, "dropped idle demo sessions");
          }
      }
  }

  #[tokio::main]
  async fn main() -> anyhow::Result<()> {
      missing_ebooks::telemetry::init();
      let cli = Cli::parse();
      let demo_config = load_config(&cli)?;

      // Resolve the scenario first, so an unknown name fails fast.
      let scenario = scenarios::find_scenario(&demo_config.scenario)
          .ok_or_else(|| anyhow::anyhow!("unknown scenario {:?}", demo_config.scenario))?;

      // Seed the scenario into a stable directory under /tmp. The data is synthetic
      // and the container is ephemeral, so it is never cleaned up explicitly. /tmp
      // matches the explore harness and keeps the root path short. It is a no-op in
      // the Linux container, where the platform temp dir is already /tmp.
      let seed_dir = std::path::Path::new("/tmp").join("missing-ebooks-demo");
      std::fs::create_dir_all(&seed_dir)?;
      let roots = scenarios::materialize(&(scenario.spec)(), &seed_dir);

      // The production config over the seeded roots, defaulted otherwise.
      // autosync_interval_seconds=0 disables the autosync loop everywhere a
      // production AppState would build one (ADR-0023). The demo never builds an
      // AppState today, so this is a placeholder that documents the choice: the
      // session sweep's idle signal does not yet track SSE traffic, so per-session
      // loops would extend sessions inappropriately. A follow-up captures
      // showcasing autosync in the demo properly.
      let config = Config {
          library_roots: roots,
          autosync_interval_seconds: 0,
          ..Default::default()
      };
      let settings = ScanSettings::compile(config.scan_inputs())?;

      let bind = demo_config.bind.clone();
      let state = Arc::new(build_state(config, settings, demo_config).await);

      tokio::spawn(run_reaper(state.clone()));

      let listener = tokio::net::TcpListener::bind(&bind).await?;
      tracing::info!(%bind, "missing-ebooks demo listening");
      let serve = axum::serve(listener, demo_router(state))
          .with_graceful_shutdown(missing_ebooks::shutdown::signal());
      serve.await?;
      Ok(())
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use clap::CommandFactory;

      #[test]
      fn demo_help_flag_displays_help() {
          let err = Cli::try_parse_from(["missing-ebooks-demo", "--help"]).unwrap_err();
          assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
      }

      #[test]
      fn demo_version_flag_displays_version() {
          let err = Cli::try_parse_from(["missing-ebooks-demo", "--version"]).unwrap_err();
          assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
      }

      #[test]
      fn demo_scenario_and_bind_are_optional() {
          let cli = Cli::try_parse_from(["missing-ebooks-demo"]).unwrap();
          assert!(cli.scenario.is_none());
          assert!(cli.bind.is_none());
      }

      #[test]
      fn demo_flags_parse() {
          let cli = Cli::try_parse_from([
              "missing-ebooks-demo",
              "--scenario", "mixed-forest",
              "--bind", "0.0.0.0:9000",
          ]).unwrap();
          assert_eq!(cli.scenario.as_deref(), Some("mixed-forest"));
          assert_eq!(cli.bind.as_deref(), Some("0.0.0.0:9000"));
      }

      #[test]
      fn demo_after_help_lists_env_vars() {
          let help = Cli::command().render_help().to_string();
          for var in [
              "DEMO_BIND",
              "DEMO_SCENARIO",
              "DEMO_MAX_SESSIONS",
              "DEMO_IDLE_SECS",
              "DEMO_COOKIE_NAME",
          ] {
              assert!(help.contains(var), "{var} missing from demo --help");
          }
      }
  }
  ```

- [ ] **Step 10: Run the demo-bin tests to verify they pass**

  ```bash
  cargo test --locked --features fixtures --bin missing-ebooks-demo -- tests::
  ```

  Expected: 5 tests pass.

- [ ] **Step 11: Smoke-test demo `--help` and `--version` end to end**

  ```bash
  cargo run --locked --features fixtures --bin missing-ebooks-demo -- --help
  echo "exit: $?"
  cargo run --locked --features fixtures --bin missing-ebooks-demo -- --version
  echo "exit: $?"
  ```

  Expected: both print and exit 0.

- [ ] **Step 12: Run the full test suite**

  ```bash
  cargo test --locked --features fixtures
  ```

  Expected: all pass.

- [ ] **Step 13: Commit**

  ```bash
  git add Cargo.toml Cargo.lock src/main.rs src/bin/demo.rs
  git commit -m "feat(cli): add --help, --version, --print-config, --config via clap

  Both binaries gain clap-derived Cli structs. missing-ebooks supports
  --print-config (prints the bundled template) and --config (with env fallback
  to MISSING_EBOOKS_CONFIG). missing-ebooks-demo supports --scenario and
  --bind, overriding DEMO_SCENARIO and DEMO_BIND. The existing env-var
  precedence inside Config::load and load_config is unchanged; flags layer
  on top.

  Audit item #4. First-time cargo install users now have an in-process
  discovery path for the env vars."
  ```

---

## Task 3: Poison-recover the demo session mutex

**Goal of this commit:** Audit item #1. A panic under the session lock no longer puts the demo into a panic-loop. Mirrors the fix already shipped for `raw_view::lock_index` and `autosync::lock_inner`.

**Files:**
- Modify: `src/demo/state.rs`, `src/demo/handlers.rs`
- Test: `src/demo/state.rs::tests` (new module)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `DemoState::lock_sessions(&self) -> std::sync::MutexGuard<'_, SessionStore>`: `pub(crate)` helper. All five `.lock().expect("session lock")` call sites route through it.

**Background.** Today five places call `state.sessions.lock().expect("session lock")`:

- `src/demo/state.rs:52`: `DemoState::reap_idle`
- `src/demo/handlers.rs:163`: `index`
- `src/demo/handlers.rs:193`: `mark`
- `src/demo/handlers.rs:237`: `reset`
- `src/demo/handlers.rs:280`: `events`

A poisoned mutex makes `.expect(...)` panic, which on a panic-on-panic-during-unwind scenario aborts the process; even without abort, every subsequent request re-panics. The fix mirrors `src/autosync.rs:350-361` exactly.

- [ ] **Step 1: Write the failing test**

  Append to `src/demo/state.rs` (no `#[cfg(test)] mod tests` exists today; create one):

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::raw_view::RawView;

      /// Build a minimal DemoState whose base_raw is empty (no roots). Enough
      /// to exercise the session-store lock. No scan runs.
      fn test_state() -> DemoState {
          DemoState {
              base_raw: Arc::new(RawView::new()),
              sessions: Mutex::new(SessionStore::new(8)),
              config: DemoConfig {
                  bind: "127.0.0.1:0".to_string(),
                  scenario: "test".to_string(),
                  max_sessions: 8,
                  idle: Duration::from_secs(60),
                  cookie_name: "me_demo_sid".to_string(),
              },
              search_links: Vec::new(),
          }
      }

      #[test]
      fn lock_sessions_recovers_from_poisoning() {
          let state = Arc::new(test_state());

          // Poison the mutex: take the guard on a worker thread, then panic.
          let poisoner = Arc::clone(&state);
          let _ = std::thread::spawn(move || {
              let _guard = poisoner.sessions.lock().unwrap();
              panic!("intentional poison for test");
          })
          .join();

          // The bare std API would now panic on .expect(...).
          assert!(
              state.sessions.lock().is_err(),
              "test setup failed: mutex was not poisoned"
          );

          // lock_sessions must recover and return a usable guard.
          let guard = state.lock_sessions();
          assert_eq!(guard.len(), 0, "recovered guard exposes the prior state");
          drop(guard);
      }
  }
  ```

- [ ] **Step 2: Run the test to verify it fails**

  ```bash
  cargo test --locked --lib demo::state::tests::lock_sessions_recovers_from_poisoning
  ```

  Expected: FAIL with "no method named `lock_sessions` found for reference `&DemoState`".

- [ ] **Step 3: Add `lock_sessions` to `DemoState`**

  In `src/demo/state.rs`, replace the existing `impl DemoState { ... }` block with:

  ```rust
  impl DemoState {
      /// How many library roots the base view carries. Bounds the root index a
      /// mark may name.
      pub(crate) fn num_roots(&self) -> usize {
          self.base_raw.len()
      }

      /// Acquire the session store lock, recovering on poison.
      ///
      /// Poison means a previous thread panicked while holding the lock. The
      /// session table itself is intact as far as the surviving thread can
      /// tell, so we proceed with a `tracing::warn` rather than propagate the
      /// panic. Mirrors `raw_view::lock_index` and `autosync::lock_inner`.
      pub(crate) fn lock_sessions(&self) -> std::sync::MutexGuard<'_, SessionStore> {
          self.sessions.lock().unwrap_or_else(|poisoned| {
              tracing::warn!("demo session mutex poisoned; recovering");
              poisoned.into_inner()
          })
      }

      /// Drop every session idle past the configured window as of `now`. Returns the
      /// number reaped. Called on a timer by the binary's reaper task.
      pub fn reap_idle(&self, now: Instant) -> usize {
          self.lock_sessions().reap_idle(now, self.config.idle)
      }
  }
  ```

  This both adds `lock_sessions` and migrates the `reap_idle` call site (the first of the five) to use it.

- [ ] **Step 4: Migrate the four handler call sites**

  In `src/demo/handlers.rs`, find each of these lines and substitute. The surrounding `let resolved = { ... };` blocks are unchanged.

  At `src/demo/handlers.rs:163` (in `index`):

  ```rust
          let mut store = state.sessions.lock().expect("session lock");
  ```

  becomes:

  ```rust
          let mut store = state.lock_sessions();
  ```

  At `src/demo/handlers.rs:193` (in `mark`): same substitution.

  At `src/demo/handlers.rs:237` (in `reset`): same substitution.

  At `src/demo/handlers.rs:280` (in `events`): same substitution.

- [ ] **Step 5: Verify no `expect("session lock")` remains**

  ```bash
  rg 'expect\("session lock"\)' src/
  ```

  Expected: zero matches. (If any remain, fix them.)

- [ ] **Step 6: Run the poison-recovery test to verify it passes**

  ```bash
  cargo test --locked --lib demo::state::tests::lock_sessions_recovers_from_poisoning
  ```

  Expected: PASS.

- [ ] **Step 7: Run the full demo test suite**

  ```bash
  cargo test --locked --lib demo::
  cargo test --locked --features fixtures
  ```

  Expected: all pass. The existing handler tests still exercise the same code paths; only the lock-acquisition shape changed.

- [ ] **Step 8: Commit**

  ```bash
  git add src/demo/state.rs src/demo/handlers.rs
  git commit -m "feat(demo): recover demo session mutex from poison

  A panic under the session lock no longer puts the demo into a panic-loop
  until the container restarts. DemoState::lock_sessions mirrors the
  recovery helpers already shipped for raw_view::lock_index and
  autosync::lock_inner: on poison, log a warn and return the inner guard.
  All five .lock().expect(\"session lock\") call sites route through it.

  Audit item #1."
  ```

---

## Task 4: Set-based mark storage with folder-existence validation

**Goal of this commit:** Audit item #2 (storage half). Per-session marks become `HashSet<MarkKey>`, bounded structurally by the scenario rather than by attacker behavior. `POST /mark` validates that `(root, rel)` names a folder in `base_raw`; unknown folders return 400 instead of accumulating garbage in the session. `derive_view` stays present and still functional during this commit by building a transient `Vec<Mark>` from the set; Task 5 deletes it.

**Files:**
- Modify: `src/demo/session.rs`, `src/demo/handlers.rs`
- Test: `src/demo/session.rs::tests` (existing module, additions), `src/demo/handlers.rs::tests` (existing module, additions)

**Interfaces:**
- Consumes: Task 3's `DemoState::lock_sessions`.
- Produces:
  - `pub type MarkKey = (usize, String, Marker);`
  - On `SessionStore`:
    - `pub fn insert_mark(&mut self, sid: &SessionId, key: MarkKey) -> bool`: returns true when newly added, false on duplicate or unknown session.
    - `pub fn remove_mark(&mut self, sid: &SessionId, key: &MarkKey) -> bool`: returns true when present and removed.
    - `pub fn marks(&self, sid: &SessionId) -> &HashSet<MarkKey>`: replaces today's `&[Mark]` accessor.
    - `pub fn clear_marks(&mut self, sid: &SessionId)`: unchanged semantics.
  - `Mark` struct is dropped. `append_mark` is dropped.
  - In `src/demo/handlers.rs`:
    - `fn folder_exists_in_base(base: &RawView, root: usize, rel: &str) -> bool`: private helper.
    - `/mark` returns `(StatusCode::BAD_REQUEST, "unknown folder")` when `folder_exists_in_base` returns false.

**Background.** `Mark { root: usize, rel: String, kind: Marker }` becomes the tuple `(usize, String, Marker)`. The wire shape (`MarkRequest` in `src/web.rs:40-49`) is unchanged; the handler now builds a `MarkKey` from a `MarkRequest` instead of building a `Mark`. The render path (`derive_view`) still wants `&[Mark]`; we adapt by building a transient `Vec<Mark>` from the set inside the handler. This is wasted work that goes away in Task 5; the equivalence test in Task 5 confirms the overlay path matches the `derive_view` output for the same set.

The `clear_marks` no-op-when-missing behavior is preserved (used by `/reset`).

The spec's `insert_mark` and `remove_mark` originally returned `Result<bool, UnknownSession>`. We simplify to `bool` because every existing call site already runs immediately after `resolve_in_store` returned `Some((sid, _))`, which guarantees the session exists; an `UnknownSession` error would only fire on a logic bug, not a user-reachable path. Matching today's `append_mark` semantics (silent no-op when missing) keeps the handler shape unchanged.

`folder_exists_in_base` follows the spec's body exactly. ADR-0005 says the root folder carries `rel_path = empty PathBuf` in `ScannedFolder` but is named `"."` on the wire, so the helper special-cases `rel == "."` to true for any walked root.

- [ ] **Step 1: Write the failing session-store tests**

  In `src/demo/session.rs`, find the existing `#[cfg(test)] mod tests { ... }` block (lines 125-219 in the current file) and add these tests inside it. If the existing tests reference the soon-to-be-deleted `Mark` and `append_mark`, leave them in place for now. Step 4 will rewrite them.

  ```rust
      #[test]
      fn insert_mark_dedupes() {
          let mut store = SessionStore::new(8);
          let sid = SessionId("s1".to_string());
          let now = Instant::now();
          store.create(sid.clone(), now).unwrap();

          let key = (0_usize, "Author/Book".to_string(), Marker::NoEbook);
          assert!(store.insert_mark(&sid, key.clone()), "first insert is new");
          assert!(!store.insert_mark(&sid, key.clone()), "second insert is a dup");
          assert_eq!(store.marks(&sid).len(), 1);
      }

      #[test]
      fn marks_set_is_per_session() {
          let mut store = SessionStore::new(8);
          let s1 = SessionId("s1".to_string());
          let s2 = SessionId("s2".to_string());
          let now = Instant::now();
          store.create(s1.clone(), now).unwrap();
          store.create(s2.clone(), now).unwrap();

          store.insert_mark(&s1, (0, "A".to_string(), Marker::NoEbook));
          assert_eq!(store.marks(&s1).len(), 1);
          assert_eq!(store.marks(&s2).len(), 0);
      }

      #[test]
      fn clear_marks_empties_the_set() {
          let mut store = SessionStore::new(8);
          let sid = SessionId("s1".to_string());
          let now = Instant::now();
          store.create(sid.clone(), now).unwrap();
          store.insert_mark(&sid, (0, "A".to_string(), Marker::NoEbook));
          store.insert_mark(&sid, (0, "B".to_string(), Marker::EbookElsewhere));
          assert_eq!(store.marks(&sid).len(), 2);

          store.clear_marks(&sid);
          assert_eq!(store.marks(&sid).len(), 0);
      }

      #[test]
      fn remove_mark_returns_whether_present() {
          let mut store = SessionStore::new(8);
          let sid = SessionId("s1".to_string());
          let now = Instant::now();
          store.create(sid.clone(), now).unwrap();
          let key = (0_usize, "A".to_string(), Marker::NoEbook);
          store.insert_mark(&sid, key.clone());

          assert!(store.remove_mark(&sid, &key), "first remove found it");
          assert!(!store.remove_mark(&sid, &key), "second remove is a no-op");
          assert_eq!(store.marks(&sid).len(), 0);
      }

      #[test]
      fn marks_on_unknown_session_is_empty() {
          let store = SessionStore::new(8);
          let sid = SessionId("nope".to_string());
          assert!(store.marks(&sid).is_empty());
      }
  ```

- [ ] **Step 2: Run the new tests to verify they fail**

  ```bash
  cargo test --locked --lib demo::session::tests::insert_mark_dedupes
  ```

  Expected: FAIL: `insert_mark` not defined, `Mark` references stale, etc.

- [ ] **Step 3: Replace `src/demo/session.rs` with the set-backed implementation**

  Replace the entire production portion (lines 1-123) with the version below. Preserve the test module from Step 1.

  ```rust
  //! In-memory session table for the demo: which cookie maps to which set of marks,
  //! and when each session was last seen. Bounded by a global cap; idle sessions
  //! are reaped on a timer. Nothing here touches disk.

  use std::collections::{HashMap, HashSet};
  use std::time::{Duration, Instant};

  use crate::marker::Marker;

  /// An opaque session id carried in the visitor's cookie.
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub struct SessionId(pub String);

  /// One mark in the session's set: the library root index, the folder path
  /// relative to that root (or "." for the root itself, per ADR-0005), and the
  /// marker kind. The set is keyed on this tuple, so repeated identical marks
  /// are no-ops at insert time and per-session size is structurally bounded by
  /// the scenario's `|markable folders x marker kinds|`.
  pub type MarkKey = (usize, String, Marker);

  /// One visitor's private state: the marks they have applied as a set, and when
  /// the session was last touched.
  struct Session {
      marks: HashSet<MarkKey>,
      last_seen: Instant,
  }

  /// Returned by `create` when the global session cap is reached.
  #[derive(Debug, PartialEq, Eq)]
  pub struct AtCapacity;

  /// The session table and its global ceiling. One process holds one of these
  /// behind a mutex. Every operation runs under that single lock.
  pub struct SessionStore {
      sessions: HashMap<SessionId, Session>,
      max_sessions: usize,
  }

  impl SessionStore {
      /// A new, empty store that admits up to `max_sessions` concurrent sessions.
      pub fn new(max_sessions: usize) -> SessionStore {
          SessionStore {
              sessions: HashMap::new(),
              max_sessions,
          }
      }

      /// How many sessions are live.
      pub fn len(&self) -> usize {
          self.sessions.len()
      }

      /// Whether no sessions are live.
      pub fn is_empty(&self) -> bool {
          self.sessions.is_empty()
      }

      /// Bump an existing session's last-seen to `now`. Returns whether the session
      /// existed; a `false` result means the cookie is unknown or was reaped.
      pub fn touch(&mut self, sid: &SessionId, now: Instant) -> bool {
          match self.sessions.get_mut(sid) {
              Some(session) => {
                  session.last_seen = now;
                  true
              }
              None => false,
          }
      }

      /// Create a fresh, empty session under the cap. Returns `Err(AtCapacity)` when
      /// the store is full, which the caller turns into the 503 page.
      pub fn create(&mut self, sid: SessionId, now: Instant) -> Result<(), AtCapacity> {
          if self.sessions.len() >= self.max_sessions {
              return Err(AtCapacity);
          }
          self.sessions.insert(
              sid,
              Session {
                  marks: HashSet::new(),
                  last_seen: now,
              },
          );
          Ok(())
      }

      /// Insert a mark into a session. Returns `true` when newly added,
      /// `false` when the mark was already present or the session is gone.
      /// Silent no-op on unknown sessions matches today's `append_mark` shape.
      /// Handlers always call this immediately after `resolve_in_store`, so
      /// the session-gone branch is a logic-bug guard, not a user-reachable path.
      pub fn insert_mark(&mut self, sid: &SessionId, key: MarkKey) -> bool {
          match self.sessions.get_mut(sid) {
              Some(session) => session.marks.insert(key),
              None => false,
          }
      }

      /// Remove a mark from a session. Returns `true` when the mark was
      /// present and removed, `false` when absent or the session is gone.
      pub fn remove_mark(&mut self, sid: &SessionId, key: &MarkKey) -> bool {
          match self.sessions.get_mut(sid) {
              Some(session) => session.marks.remove(key),
              None => false,
          }
      }

      /// Empty a session's marks, leaving the session in place. A no-op when the
      /// session is gone.
      pub fn clear_marks(&mut self, sid: &SessionId) {
          if let Some(session) = self.sessions.get_mut(sid) {
              session.marks.clear();
          }
      }

      /// The marks a session has applied as a set. Empty when the session is
      /// unknown. Borrowed for the duration of the caller's lock guard. The
      /// render path consumes this reference directly without copying.
      pub fn marks(&self, sid: &SessionId) -> &HashSet<MarkKey> {
          // A static empty set so the unknown-session path can return a
          // `&HashSet` without a per-call allocation. `OnceLock` keeps it
          // const-eval-free without an unsafe `static mut` or a per-call
          // `Box::leak`.
          static EMPTY: std::sync::OnceLock<HashSet<MarkKey>> = std::sync::OnceLock::new();
          self.sessions
              .get(sid)
              .map(|session| &session.marks)
              .unwrap_or_else(|| EMPTY.get_or_init(HashSet::new))
      }

      /// Drop every session idle for at least `idle` as of `now`; returns how many
      /// were dropped.
      pub fn reap_idle(&mut self, now: Instant, idle: Duration) -> usize {
          let before = self.sessions.len();
          self.sessions
              .retain(|_, session| now.duration_since(session.last_seen) < idle);
          before - self.sessions.len()
      }
  }
  ```

  The `OnceLock` for `EMPTY` keeps the `&HashSet<MarkKey>` return signature without per-call allocation. Alternative: change `marks` to return `Option<&HashSet<MarkKey>>` and have callers handle `None`. We pick `&HashSet` because every caller wants the empty-set fallback (the render path needs to iterate, and a missing session is logically "no marks").

- [ ] **Step 4: Rewrite the existing session tests that reference `Mark` and `append_mark`**

  The existing `#[cfg(test)] mod tests` in `src/demo/session.rs` had tests written against `Mark` and `append_mark`. Open the file, find each test that constructs `Mark { ... }` or calls `.append_mark(...)`, and convert it. The shape change is mechanical:

  - `Mark { root: r, rel: s.to_string(), kind: k }` becomes `(r, s.to_string(), k)`.
  - `store.append_mark(&sid, Mark { ... })` becomes `store.insert_mark(&sid, (r, s.to_string(), k));` (discard the bool unless the test cares).
  - `store.marks(&sid)` previously returned `&[Mark]`; now returns `&HashSet<MarkKey>`. Tests that asserted on length adapt directly (`.len()` works on both). Tests that asserted on order (insertion-order replay) need to assert on set membership instead; flag any such test and rewrite it as a contains check.

  Run the session tests:

  ```bash
  cargo test --locked --lib demo::session::
  ```

  Expected: all pass (existing tests adapted, new tests from Step 1 added).

- [ ] **Step 5: Write the failing handler-validation tests**

  In `src/demo/handlers.rs`, find the existing `#[cfg(test)] mod tests` block (starts around line 312). Add these tests inside it. They use the same `oneshot` pattern as `a_mark_persists_across_requests_within_a_session` (which lives at `src/demo/handlers.rs:385-422`); copy the setup helper from there if one exists, otherwise inline the build.

  ```rust
      /// Out-of-range root still returns 400. The existing path's shape must
      /// not regress.
      #[tokio::test]
      async fn mark_rejects_unknown_root() {
          let state = build_test_state().await;
          let app = router(state);
          let response = app
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/mark")
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=99&rel=Author/Book&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
          let body = http_body_util::BodyExt::collect(response.into_body())
              .await
              .unwrap()
              .to_bytes();
          assert_eq!(&body[..], b"unknown library root");
      }

      #[tokio::test]
      async fn mark_rejects_unknown_rel() {
          let state = build_test_state().await;
          let app = router(state);
          let response = app
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/mark")
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=0&rel=Not/A/Real/Folder&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
          let body = http_body_util::BodyExt::collect(response.into_body())
              .await
              .unwrap()
              .to_bytes();
          assert_eq!(&body[..], b"unknown folder");
      }

      #[tokio::test]
      async fn mark_accepts_root_dot_mark() {
          // ADR-0005: every walked root is itself flaggable, named "." on the wire.
          let state = build_test_state().await;
          let app = router(state);
          let response = app
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/mark")
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=0&rel=.&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(response.status(), axum::http::StatusCode::OK);
      }
  ```

  The `build_test_state` helper: if one already exists in the test module, use it. Otherwise add it at the top of the test module. The shape:

  ```rust
      async fn build_test_state() -> Arc<DemoState> {
          use crate::config::Config;
          use crate::scanner::ScanSettings;
          use crate::demo::state::{DemoConfig, build_state};
          use std::time::Duration;

          let dir = tempfile::tempdir().unwrap();
          // One flagged folder under root index 0 so /mark has a target.
          std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
          std::fs::File::create(dir.path().join("Author/Book/01.mp3")).unwrap();
          let cfg = Config {
              library_roots: vec![dir.path().to_path_buf()],
              ttl_seconds: 60,
              autosync_interval_seconds: 0,
              ..Config::default()
          };
          let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
          let demo_cfg = DemoConfig {
              bind: "127.0.0.1:0".to_string(),
              scenario: "test".to_string(),
              max_sessions: 8,
              idle: Duration::from_secs(60),
              cookie_name: "me_demo_sid".to_string(),
              };
          // tempdir must outlive the state. Leak it for the test's lifetime.
          // Per-test tempdirs in this module follow the same pattern (see
          // existing tests).
          let _ = Box::leak(Box::new(dir));
          Arc::new(build_state(cfg, settings, demo_cfg).await)
      }
  ```

  If the existing test module already has a different helper name (e.g. `make_state`), reuse it and adapt the new tests' first line.

- [ ] **Step 6: Run the new handler tests to verify they fail**

  ```bash
  cargo test --locked --lib demo::handlers::tests::mark_rejects_unknown_rel
  ```

  Expected: FAIL (returns 200 instead of 400, because validation is not yet wired).

- [ ] **Step 7: Add `folder_exists_in_base` and wire it into `mark`**

  In `src/demo/handlers.rs`, add the helper near the top of the file (above `derive_view`), and update the `mark` handler. The full updated `mark` handler:

  Add to the imports at the top:

  ```rust
  use std::path::PathBuf;
  use crate::scanner;
  ```

  Add the helper above `derive_view`:

  ```rust
  /// Whether the (root, rel) pair names a folder the base view actually walked.
  /// Gates `/mark` and `/unmark` so garbage paths cannot enter the session set.
  ///
  /// `rel == "."` is true for any walked root, per ADR-0005 (the root carries
  /// an empty `rel_path` in `ScannedFolder`, not "."). For non-root cases the
  /// `rel` string is compared component-aware via `PathBuf` equality.
  ///
  /// O(F) per call. Runs at most twice per user click (mark, later unmark). On
  /// the biggest scenario this is sub-millisecond.
  fn folder_exists_in_base(base: &RawView, root: usize, rel: &str) -> bool {
      let Some(scanner::RootScan::Walked { folders, .. }) = base.get(root) else {
          return false;
      };
      if rel == "." {
          return true;
      }
      let target = PathBuf::from(rel);
      folders.iter().any(|f| f.rel_path == target)
  }
  ```

  Replace the existing `mark` handler body (currently at `src/demo/handlers.rs:176-218`) with:

  ```rust
  async fn mark(
      State(state): State<Arc<DemoState>>,
      headers: HeaderMap,
      Form(req): Form<MarkRequest>,
  ) -> Response {
      let mode = req.view;
      // The UI only ever submits a root index from a rendered button, so an
      // out-of-range index is a malformed request.
      if req.root >= state.num_roots() {
          return (StatusCode::BAD_REQUEST, "unknown library root").into_response();
      }
      // Reject paths that do not exist in the base view, so garbage marks
      // never reach the session set. Audit item #2: caps per-session size
      // structurally at `|markable folders x marker kinds|`.
      if !folder_exists_in_base(&state.base_raw, req.root, &req.rel) {
          return (StatusCode::BAD_REQUEST, "unknown folder").into_response();
      }
      let now = Instant::now();
      let existing = read_cookie(&headers, &state.config.cookie_name);
      let resolved = {
          let mut store = state.lock_sessions();
          match resolve_in_store(&mut store, &state.config, existing, now) {
              Some((sid, set_cookie)) => {
                  store.insert_mark(&sid, (req.root, req.rel.clone(), req.kind));
                  // Transient Vec<Mark> for derive_view. Task 5 deletes this.
                  let marks = marks_for_render(&store, &sid);
                  Some((set_cookie, marks))
              }
              None => None,
          }
      };
      let Some((set_cookie, marks)) = resolved else {
          return capacity_response();
      };
      let view = derive_view(&state.base_raw, &marks, mode);
      let markup = render_section(&view[req.root], req.root, None, &state.search_links, mode);
      let mut response = Html(markup.into_string()).into_response();
      if let Some(cookie) = set_cookie {
          response.headers_mut().append(header::SET_COOKIE, cookie);
      }
      response
  }
  ```

  Add the transient-conversion helper near the top of the file (above `derive_view`). It is `#[allow(dead_code)]`-free because every handler in this commit uses it; Task 5 deletes it together with `derive_view`:

  ```rust
  /// Adapt the session's set to the legacy `Vec<Mark>` shape that
  /// `derive_view` still consumes. Iterates `Marker::ALL` in declaration order
  /// so the resulting cover_files order is deterministic. Task 5 deletes this
  /// helper when the overlay path replaces `derive_view`.
  fn marks_for_render(store: &SessionStore, sid: &SessionId) -> Vec<Mark> {
      let mut keys: Vec<&MarkKey> = store.marks(sid).iter().collect();
      // Stable order: by (root, rel, Marker::ALL index). Keeps the legacy
      // derive_view output deterministic across this commit.
      keys.sort_by(|a, b| {
          a.0.cmp(&b.0)
              .then_with(|| a.1.cmp(&b.1))
              .then_with(|| marker_order(a.2).cmp(&marker_order(b.2)))
      });
      keys.into_iter()
          .map(|(root, rel, kind)| Mark {
              root: *root,
              rel: rel.clone(),
              kind: *kind,
          })
          .collect()
  }

  fn marker_order(m: Marker) -> usize {
      Marker::ALL.iter().position(|x| *x == m).unwrap_or(0)
  }
  ```

  Note: `Mark` is still defined in `src/demo/session.rs` at this point; keep it. Step 8 deletes `Mark` once nothing references it; **but `derive_view` still wants `&[Mark]`, so we re-add `Mark` as a private helper struct in `handlers.rs`** for this commit only. Add at the top of `handlers.rs`:

  ```rust
  /// Transient legacy mark shape used to feed `derive_view` during the storage
  /// migration. Task 5 deletes this together with `derive_view`.
  struct Mark {
      root: usize,
      rel: String,
      kind: Marker,
  }
  ```

  Add `use crate::marker::Marker;` to the imports if not already present.

  **Then delete `Mark` and `append_mark` from `src/demo/session.rs`**; Step 3's replacement file already omits them, so this is automatic if Step 3 was followed verbatim. Confirm by ripgrep:

  ```bash
  rg 'struct Mark|append_mark' src/demo/session.rs
  ```

  Expected: zero matches.

- [ ] **Step 8: Update the `index`, `reset`, and `events` handlers to use `marks_for_render`**

  The four handler call sites that previously did `store.marks(&sid).to_vec()` (which returned `Vec<Mark>`) must now go through `marks_for_render(&store, &sid)`. The shape:

  In `index` at `src/demo/handlers.rs:163-167`, the line:

  ```rust
          resolve_in_store(&mut store, &state.config, existing, now)
              .map(|(sid, set_cookie)| (set_cookie, store.marks(&sid).to_vec()))
  ```

  becomes:

  ```rust
          resolve_in_store(&mut store, &state.config, existing, now)
              .map(|(sid, set_cookie)| (set_cookie, marks_for_render(&store, &sid)))
  ```

  Same substitution in `events` at `src/demo/handlers.rs:280-284`. The `mark` handler is already updated in Step 7. The `reset` handler does not need `marks_for_render`; it only calls `clear_marks` and does not render.

- [ ] **Step 9: Update imports in `handlers.rs`**

  The line `use super::session::{AtCapacity, Mark, SessionId, SessionStore};` (currently at `src/demo/handlers.rs:21`) becomes:

  ```rust
  use super::session::{AtCapacity, MarkKey, SessionId, SessionStore};
  ```

  `Mark` is now a private struct inside `handlers.rs` (from Step 7); `MarkKey` is the public type alias on the wire.

- [ ] **Step 10: Run the full test suite**

  ```bash
  cargo test --locked --features fixtures
  ```

  Expected: all pass. The new handler tests (Step 5) pass. The existing handler tests continue to pass: they exercise `/mark` and `/events` end-to-end and are agnostic about storage shape.

  If a handler test that used to expect order-dependent behavior (e.g., asserting on a specific cover_files order) breaks, the fix is to make the test order-independent via set comparison or by feeding marks in `Marker::ALL` order.

- [ ] **Step 11: Run clippy**

  ```bash
  cargo clippy --locked --all-targets --features fixtures -- -D warnings
  ```

  Expected: clean. Common warnings to fix: `dead_code` on `marker_order` if it ends up unused (shouldn't: it's used by `marks_for_render`), or `redundant_clone` if `req.rel.clone()` is flagged (keep it; the borrow is consumed by both the validation and the insert).

- [ ] **Step 12: Commit**

  ```bash
  git add src/demo/session.rs src/demo/handlers.rs
  git commit -m "refactor(demo): store marks as a set, validate folder existence at /mark

  Per-session storage moves from Vec<Mark> to HashSet<MarkKey>, so repeated
  identical marks are no-ops at insert time and per-session size is bounded
  structurally by the scenario's |markable folders x marker kinds|. POST /mark
  now validates (root, rel) against the base view and returns 400 'unknown
  folder' for paths the scanner did not walk, so attacker-supplied garbage
  never reaches the set.

  derive_view stays in place during this commit; the handler builds a
  transient Vec<Mark> from the set via marks_for_render so the legacy render
  shape still works. The next commit deletes both via the overlay path.

  Audit item #2 (storage half). The render-cost half lands in the next
  commit."
  ```

---

## Task 5: MarkOverlay render path, delete `derive_view`, add `POST /unmark`

**Goal of this commit:** Audit item #2 (render half) and the inline `/unmark` fix. Per-request render cost drops from `O((M+1) × F)` to `O(F × depth)` by walking the base view once and consulting a `MarkOverlay` per folder. `derive_view` is deleted. The demo toast's Undo button stops silently 404-ing.

**Files:**
- Create: `src/demo/overlay.rs`
- Modify: `src/demo.rs` (add `pub mod overlay;`), `src/demo/handlers.rs`
- Test: `src/demo/overlay.rs::tests` (in-file), `src/demo/handlers.rs::tests` (additions)

**Interfaces:**
- Consumes: Task 4's `MarkKey`, `SessionStore::marks`, `SessionStore::insert_mark`, `SessionStore::remove_mark`, `folder_exists_in_base`, `Marker::ALL`, and the production `package_view`/`render_section`/`render_view`/`FlaggedView` from `crate::web::render`.
- Produces:
  - `pub struct MarkOverlay<'a> { marks: &'a HashSet<MarkKey> }`
  - `pub fn new(marks: &'a HashSet<MarkKey>) -> Self`
  - `pub fn effective_state(&self, root: usize, rel: &Path) -> EffectiveState`
  - `pub struct EffectiveState { pub cleared_by_ancestor: bool, pub exact_markers: Vec<Marker> }`
  - `pub fn package_view_with_overlay(base: &RawView, overlay: &MarkOverlay<'_>, mode: ViewMode) -> FlaggedView`
  - Demo router gains `POST /unmark` wired to a new `unmark` handler.

**Background.** `apply_mark_raw` at `src/raw_view.rs:26-55` is the semantic oracle. For non-root mark `(root, P, marker)`:

- The folder whose `rel_path == P`: `missing_ebook = false` and `add_marker` (dedup) appends the marker filename to `cover_files`.
- Every folder whose `rel_path.starts_with(&P)` (component-aware): `missing_ebook = false` only.

For root mark `(root, ".", marker)` (per ADR-0005):

- Every folder under that root: `missing_ebook = false`.
- The empty-rel-path folder additionally gains the marker filename.

The overlay reverses this view: for each folder F, walk F's ancestors (including F itself). For each ancestor A, probe `(root, ancestor_key, marker)` in the set across all `Marker::ALL`. If any probe hits, F is cleared. If a probe hits at A == F, the marker filename is appended to F's `cover_files`. The empty path maps to the wire key `"."`.

`Path::ancestors()` for `Path::new("Author/Book")` yields `Author/Book`, `Author`, `""`. For `Path::new("")` it yields `""`. The empty ancestor maps to `"."` for the lookup.

`cover_files` ordering: `apply_mark_raw` calls `add_marker` in the order marks are replayed. To match byte-for-byte, the overlay emits marker filenames in `Marker::ALL` declaration order, and the equivalence test feeds `derive_view`'s `Vec<Mark>` in the same canonical order (handled by `marks_for_render` from Task 4).

The render path is `package_view_with_overlay → render_view`. The production `package_view` lives in `src/web/render.rs:37`; we don't reuse it, we mirror its shape over the overlay. Both functions build `FlaggedView = Vec<RootSection>` (see `src/web/render.rs:17`).

A pragmatic choice: instead of fully reimplementing `package_view`'s logic over the overlay (which would duplicate the mode-filter and forest-build code from `src/web/render.rs`), we synthesize a *materialized* `RawView` by walking `base` once and applying the overlay's per-folder edits to a clone of each `ScannedFolder`, then call the production `package_view` on the result. This is still `O(F)` per render plus the overlay probes (`O(F × depth)` total), and it sidesteps risk of drift between two parallel renderers.

This is a deliberate departure from the spec's "two parallel renderers" sketch in section "Render path". The simpler shape: one renderer (the production one), and `package_view_with_overlay` is a thin wrapper that produces the synthesized `RawView` and forwards.

- [ ] **Step 1: Write the failing equivalence test (Group 1, the merge gate)**

  Create `src/demo/overlay.rs` with the test module skeleton at the bottom, but leave the production code body empty (or stubbed) so the test compiles and fails. Write the test first so the production code body in Step 2 has a clear target.

  Initial file body:

  ```rust
  //! The MarkOverlay: a borrowing view over the session's mark set that the
  //! demo render path consults per folder, replacing the clone-and-replay
  //! `derive_view` shape. The semantic oracle is `crate::raw_view::apply_mark_raw`;
  //! the equivalence test pins byte-for-byte parity.

  use std::collections::HashSet;
  use std::path::Path;

  use crate::demo::session::MarkKey;
  use crate::marker::Marker;
  use crate::raw_view::RawView;
  use crate::tree::ViewMode;
  use crate::web::render::FlaggedView;

  pub struct MarkOverlay<'a> {
      marks: &'a HashSet<MarkKey>,
  }

  impl<'a> MarkOverlay<'a> {
      pub fn new(marks: &'a HashSet<MarkKey>) -> Self {
          Self { marks }
      }

      pub fn effective_state(&self, _root: usize, _rel: &Path) -> EffectiveState {
          unimplemented!("Step 2")
      }
  }

  #[derive(Default, Debug, PartialEq, Eq)]
  pub struct EffectiveState {
      pub cleared_by_ancestor: bool,
      pub exact_markers: Vec<Marker>,
  }

  pub fn package_view_with_overlay(
      _base: &RawView,
      _overlay: &MarkOverlay<'_>,
      _mode: ViewMode,
  ) -> FlaggedView {
      unimplemented!("Step 2")
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::config::Config;
      use crate::demo::session::MarkKey;
      use crate::scanner::ScanSettings;
      use crate::scenarios;
      use crate::state::AppState;
      use crate::web::render::{package_view, render_view};
      use std::sync::Arc;

      /// For each interesting scenario and mark set: compare the overlay
      /// render to a fresh `apply_mark_raw`-replay render. The HTML must be
      /// byte-equal. This is the merge gate for Task 5: if it fails, do not
      /// land.
      #[tokio::test]
      async fn overlay_matches_replay_render_byte_for_byte() {
          for scenario_name in [
              "mixed-forest",
              "messy-shelf",
              "root-flagged",
              "pre-marked",
          ] {
              for case in interesting_mark_sets(scenario_name) {
                  assert_byte_equal(scenario_name, &case).await;
              }
          }
      }

      struct Case {
          name: &'static str,
          marks: Vec<MarkKey>,
      }

      fn interesting_mark_sets(_scenario: &str) -> Vec<Case> {
          // Returns logical mark-sets. The fixtures-bound rel paths must
          // exist in the scenario. A missing path surfaces a clear "unknown
          // folder" failure rather than a silent skip. All scenarios share
          // a common shape with `Author/Book` paths under root 0. The
          // pre-marked scenario also exercises root 1.
          vec![
              Case { name: "empty", marks: vec![] },
              Case {
                  name: "single_leaf",
                  marks: vec![(0, "Author/Book".to_string(), Marker::NoEbook)],
              },
              Case {
                  name: "single_root_dot",
                  // ADR-0005 root mark.
                  marks: vec![(0, ".".to_string(), Marker::NoEbook)],
              },
              Case {
                  name: "ancestor_plus_descendant",
                  marks: vec![
                      (0, "Author".to_string(), Marker::NoEbook),
                      (0, "Author/Book".to_string(), Marker::EbookElsewhere),
                  ],
              },
              Case {
                  name: "both_markers_on_one_folder",
                  marks: vec![
                      (0, "Author/Book".to_string(), Marker::NoEbook),
                      (0, "Author/Book".to_string(), Marker::EbookElsewhere),
                  ],
              },
          ]
      }

      async fn assert_byte_equal(scenario_name: &str, case: &Case) {
          let dir = tempfile::tempdir().unwrap();
          let scenario = scenarios::find_scenario(scenario_name).expect("scenario exists");
          let roots = scenarios::materialize(&(scenario.spec)(), dir.path());
          let config = Config {
              library_roots: roots,
              ttl_seconds: 600,
              ..Config::default()
          };
          let links = config.search_links.clone();
          let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
          let state = Arc::new(AppState::new(config, settings));
          // Warm the cache so .current() returns a stable raw view.
          let base = state.store.current().await;

          for mode in [ViewMode::GapsOnly, ViewMode::ShowAll] {
              // Path A: replay marks via apply_mark_raw, then package_view +
              // render_view. This is the production-equivalent path.
              let mut raw_replay = (*base).clone();
              // Filter for valid folder rels so a missing-from-scenario mark
              // does not silently no-op the replay. The equivalence is only
              // meaningful when both paths see the same logical state.
              let valid_marks: Vec<&MarkKey> = case
                  .marks
                  .iter()
                  .filter(|(root, rel, _)| folder_in_raw(&raw_replay, *root, rel))
                  .collect();
              for (root, rel, kind) in &valid_marks {
                  crate::raw_view::apply_mark_raw(&mut raw_replay, *root, rel, *kind);
              }
              let replay_view = package_view(&raw_replay, mode);
              let replay_html = render_view(&replay_view, &links, mode).into_string();

              // Path B: same logical state via the overlay.
              let mark_set: HashSet<MarkKey> = valid_marks.iter().map(|k| (*k).clone()).collect();
              let overlay = MarkOverlay::new(&mark_set);
              let overlay_view = package_view_with_overlay(&base, &overlay, mode);
              let overlay_html = render_view(&overlay_view, &links, mode).into_string();

              assert_eq!(
                  replay_html, overlay_html,
                  "scenario={scenario_name} case={} mode={mode:?}: overlay HTML diverges from replay HTML",
                  case.name
              );
          }
      }

      fn folder_in_raw(raw: &RawView, root: usize, rel: &str) -> bool {
          let Some(crate::scanner::RootScan::Walked { folders, .. }) = raw.get(root) else {
              return false;
          };
          if rel == "." {
              return true;
          }
          let target = std::path::PathBuf::from(rel);
          folders.iter().any(|f| f.rel_path == target)
      }
  }
  ```

  Register the module by editing `src/demo.rs`:

  ```rust
  //! In-memory per-session demo of the server. One process serves every visitor.
  //! Each visitor's marks live in memory keyed by a session cookie and never touch
  //! disk.

  pub mod banner;
  pub mod handlers;
  pub mod overlay;
  pub mod session;
  pub mod state;
  ```

- [ ] **Step 2: Run the equivalence test to verify it fails**

  ```bash
  cargo test --locked --features fixtures --lib demo::overlay::tests::overlay_matches_replay_render_byte_for_byte
  ```

  Expected: FAIL with `unimplemented!("Step 2")`. The skeleton compiles; the body is not implemented.

- [ ] **Step 3: Implement `effective_state` and `package_view_with_overlay`**

  Replace the stub bodies in `src/demo/overlay.rs` with:

  ```rust
  impl<'a> MarkOverlay<'a> {
      pub fn new(marks: &'a HashSet<MarkKey>) -> Self {
          Self { marks }
      }

      /// Compute the overlay-corrected state for the folder at `(root, rel)`.
      ///
      /// Walks `rel`'s ancestors (including itself) and probes every
      /// `Marker::ALL` kind. A hit on any ancestor sets `cleared_by_ancestor`.
      /// A hit on `rel` itself also appends the marker to `exact_markers` in
      /// `Marker::ALL` declaration order, matching `apply_mark_raw`'s
      /// `add_marker` output for the same canonical replay order (which
      /// `marks_for_render` enforces).
      ///
      /// Depth is typically 2-3 in audiobook libraries, so this is `O(depth)`
      /// `HashSet` probes per folder.
      pub fn effective_state(&self, root: usize, rel: &Path) -> EffectiveState {
          let mut state = EffectiveState::default();

          for ancestor in rel.ancestors() {
              let ancestor_key: String = if ancestor.as_os_str().is_empty() {
                  ".".to_string()
              } else {
                  match ancestor.to_str() {
                      Some(s) => s.to_string(),
                      None => continue,
                  }
              };

              // Iterate Marker::ALL in declaration order so the exact_markers
              // vec, when consumed by package_view_with_overlay, appends to
              // cover_files in the same order apply_mark_raw would.
              for kind in Marker::ALL {
                  let key = (root, ancestor_key.clone(), kind);
                  if self.marks.contains(&key) {
                      state.cleared_by_ancestor = true;
                      if ancestor == rel {
                          state.exact_markers.push(kind);
                      }
                  }
              }
          }

          state
      }
  }

  /// Materialize the overlay against `base` into a fresh `RawView`, then call
  /// the production `package_view`. Walks `base` once, cloning each section
  /// and applying per-folder overlay edits in place. Avoids reimplementing
  /// `package_view`'s mode-filter and forest-build logic, so the overlay path
  /// inherits any future refinement to `package_view` for free.
  ///
  /// Cost: `O(F)` clone plus `O(F * depth)` overlay probes, vs. the old
  /// `derive_view`'s `O((M+1) * F)`.
  pub fn package_view_with_overlay(
      base: &RawView,
      overlay: &MarkOverlay<'_>,
      mode: ViewMode,
  ) -> FlaggedView {
      let mut synthesized = base.clone();
      for (root_idx, section) in synthesized.iter_mut().enumerate() {
          let crate::scanner::RootScan::Walked { folders, .. } = section else {
              continue;
          };
          for folder in folders.iter_mut() {
              let state = overlay.effective_state(root_idx, &folder.rel_path);
              if state.cleared_by_ancestor {
                  folder.missing_ebook = false;
              }
              for marker in state.exact_markers {
                  let name = marker.filename().to_string();
                  if !folder.cover_files.iter().any(|existing| existing == &name) {
                      folder.cover_files.push(name);
                  }
              }
          }
      }
      crate::web::render::package_view(&synthesized, mode)
  }
  ```

  The dedup of `cover_files` mirrors `raw_view::add_marker` (`src/raw_view.rs:57-62`) literally: same dedup-against-existing-string check, same push semantics.

- [ ] **Step 4: Run the equivalence test to verify it passes**

  ```bash
  cargo test --locked --features fixtures --lib demo::overlay::tests::overlay_matches_replay_render_byte_for_byte
  ```

  Expected: PASS. If it fails on a specific scenario/case/mode, the diagnosis ladder from the spec's "Risk" section applies:
  - Diff the two HTML outputs locally (write each to a temp file) and read the first divergence.
  - If the divergence is in a folder that should have been cleared but is not, the bug is in `effective_state`'s ancestor walk.
  - If the divergence is in a folder's `cover_files` order, the bug is the iteration order in `effective_state` (must be `Marker::ALL` declaration order).
  - If the divergence is in a field of `RootSection` not visible to the overlay path (e.g., `total_audiobooks`), it means `package_view` reads a field of `ScannedFolder` that the synthesized clone did not refresh; which would be surprising given we forwarded to `package_view` directly. Re-read the `package_view` body to identify the field.

- [ ] **Step 5: Write the failing `/unmark` route test and the round-trip test**

  In `src/demo/handlers.rs`'s test module, add:

  ```rust
      #[tokio::test]
      async fn unmark_rejects_unknown_root() {
          let state = build_test_state().await;
          let app = router(state);
          let response = app
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/unmark")
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=99&rel=Author/Book&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
      }

      #[tokio::test]
      async fn unmark_rejects_unknown_rel() {
          let state = build_test_state().await;
          let app = router(state);
          let response = app
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/unmark")
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=0&rel=Not/A/Real/Folder&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
      }

      #[tokio::test]
      async fn unmark_no_op_when_not_marked() {
          // Hitting /unmark on a real folder that was never marked returns
          // 200 with the section re-rendered as if no marks existed.
          let state = build_test_state().await;
          let app = router(state);
          let response = app
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/unmark")
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=0&rel=Author/Book&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(response.status(), axum::http::StatusCode::OK);
      }

      /// Mirrors render.rs's render_is_byte_equal_across_hits_and_a_mark_undo_round_trip.
      /// POST /mark then POST /unmark on the same folder must produce a section that is
      /// byte-equal to a fresh render with no marks at all.
      #[tokio::test]
      async fn mark_then_unmark_round_trip_renders_pre_mark_state() {
          let state = build_test_state().await;
          let app = router(state.clone());

          // Round trip: /mark, then /unmark on the same key. The post-unmark
          // section must be byte-equal to a fresh session's render of the same
          // section, which uses a second app over a second state to avoid
          // session bleed.

          let cookie = "me_demo_sid=roundtripsession00000000000000".to_string();

          // First /mark establishes the session and applies one mark.
          let marked = app
              .clone()
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/mark")
                      .header("cookie", &cookie)
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=0&rel=Author/Book&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(marked.status(), axum::http::StatusCode::OK);

          // /unmark removes it.
          let unmarked = app
              .clone()
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/unmark")
                      .header("cookie", &cookie)
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=0&rel=Author/Book&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(unmarked.status(), axum::http::StatusCode::OK);
          let unmarked_body = http_body_util::BodyExt::collect(unmarked.into_body())
              .await
              .unwrap()
              .to_bytes();

          // Baseline: a fresh session renders the same section identically.
          // A second app over a second state avoids any session bleed.
          let baseline_state = build_test_state().await;
          let baseline_app = router(baseline_state);
          let baseline = baseline_app
              .oneshot(
                  axum::http::Request::builder()
                      .method("POST")
                      .uri("/unmark")
                      .header("cookie", "me_demo_sid=baselinesession0000000000000000")
                      .header("content-type", "application/x-www-form-urlencoded")
                      .body(axum::body::Body::from(
                          "root=0&rel=Author/Book&kind=no_ebook&view=gaps",
                      ))
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(baseline.status(), axum::http::StatusCode::OK);
          let baseline_body = http_body_util::BodyExt::collect(baseline.into_body())
              .await
              .unwrap()
              .to_bytes();

          assert_eq!(unmarked_body, baseline_body, "round-trip section diverges from pristine render");
      }
  ```

  Run them to verify failure:

  ```bash
  cargo test --locked --features fixtures --lib demo::handlers::tests::unmark_
  cargo test --locked --features fixtures --lib demo::handlers::tests::mark_then_unmark_round_trip
  ```

  Expected: FAIL: no `/unmark` route exists.

- [ ] **Step 6: Add the `/unmark` handler and wire it into the router**

  In `src/demo/handlers.rs`, add the handler near `mark`:

  ```rust
  async fn unmark(
      State(state): State<Arc<DemoState>>,
      headers: HeaderMap,
      Form(req): Form<MarkRequest>,
  ) -> Response {
      let mode = req.view;
      if req.root >= state.num_roots() {
          return (StatusCode::BAD_REQUEST, "unknown library root").into_response();
      }
      if !folder_exists_in_base(&state.base_raw, req.root, &req.rel) {
          return (StatusCode::BAD_REQUEST, "unknown folder").into_response();
      }
      let now = Instant::now();
      let existing = read_cookie(&headers, &state.config.cookie_name);
      let resolved = {
          let mut store = state.lock_sessions();
          match resolve_in_store(&mut store, &state.config, existing, now) {
              Some((sid, set_cookie)) => {
                  store.remove_mark(&sid, &(req.root, req.rel.clone(), req.kind));
                  // Render off the borrowed set directly via the overlay.
                  let marks = store.marks(&sid).clone();
                  Some((set_cookie, marks))
              }
              None => None,
          }
      };
      let Some((set_cookie, marks)) = resolved else {
          return capacity_response();
      };
      let overlay = crate::demo::overlay::MarkOverlay::new(&marks);
      let view = crate::demo::overlay::package_view_with_overlay(&state.base_raw, &overlay, mode);
      let markup = render_section(&view[req.root], req.root, None, &state.search_links, mode);
      let mut response = Html(markup.into_string()).into_response();
      if let Some(cookie) = set_cookie {
          response.headers_mut().append(header::SET_COOKIE, cookie);
      }
      response
  }
  ```

  Add the route to `router`:

  ```rust
  pub fn router(state: Arc<DemoState>) -> Router {
      Router::new()
          .route("/", get(index))
          .route("/mark", post(mark))
          .route("/unmark", post(unmark))
          .route("/reset", post(reset))
          .route("/rescan", post(rescan))
          .route("/events", get(events))
          .route("/healthz", get(healthz))
          .route("/static/htmx.min.js", get(htmx_script))
          .route("/static/htmx-sse.js", get(htmx_sse_script))
          .route("/static/app.css", get(app_css))
          .route("/static/app.js", get(app_js))
          .with_state(state)
  }
  ```

  Note `marks.clone()` in the handler: we clone the `HashSet<MarkKey>` once after dropping the lock guard, so the overlay borrows from a stack-local owned set. The clone is cheap (typically tens of entries) and lets the lock release before rendering.

- [ ] **Step 7: Migrate `index`, `mark`, and `events` to the overlay**

  Replace each handler's render path so it calls the overlay instead of `derive_view`. The pattern (already used in `unmark` above):

  ```rust
      // After resolved Some(...) and we have `marks: HashSet<MarkKey>`:
      let overlay = crate::demo::overlay::MarkOverlay::new(&marks);
      let view = crate::demo::overlay::package_view_with_overlay(&state.base_raw, &overlay, mode);
  ```

  In `index` (currently `src/demo/handlers.rs:154-174`), replace the body's render section. Old shape (after Task 4):

  ```rust
      let resolved = {
          let mut store = state.lock_sessions();
          resolve_in_store(&mut store, &state.config, existing, now)
              .map(|(sid, set_cookie)| (set_cookie, marks_for_render(&store, &sid)))
      };
      let Some((set_cookie, marks)) = resolved else {
          return capacity_response();
      };
      let view = derive_view(&state.base_raw, &marks, mode);
  ```

  New shape:

  ```rust
      let resolved = {
          let mut store = state.lock_sessions();
          resolve_in_store(&mut store, &state.config, existing, now)
              .map(|(sid, set_cookie)| (set_cookie, store.marks(&sid).clone()))
      };
      let Some((set_cookie, marks)) = resolved else {
          return capacity_response();
      };
      let overlay = crate::demo::overlay::MarkOverlay::new(&marks);
      let view = crate::demo::overlay::package_view_with_overlay(&state.base_raw, &overlay, mode);
  ```

  In `mark` (currently as written in Task 4 Step 7), replace the render block in the same way: after `insert_mark`, clone the set, drop the guard, build the overlay, call `package_view_with_overlay`. Full updated body:

  ```rust
  async fn mark(
      State(state): State<Arc<DemoState>>,
      headers: HeaderMap,
      Form(req): Form<MarkRequest>,
  ) -> Response {
      let mode = req.view;
      if req.root >= state.num_roots() {
          return (StatusCode::BAD_REQUEST, "unknown library root").into_response();
      }
      if !folder_exists_in_base(&state.base_raw, req.root, &req.rel) {
          return (StatusCode::BAD_REQUEST, "unknown folder").into_response();
      }
      let now = Instant::now();
      let existing = read_cookie(&headers, &state.config.cookie_name);
      let resolved = {
          let mut store = state.lock_sessions();
          match resolve_in_store(&mut store, &state.config, existing, now) {
              Some((sid, set_cookie)) => {
                  store.insert_mark(&sid, (req.root, req.rel.clone(), req.kind));
                  let marks = store.marks(&sid).clone();
                  Some((set_cookie, marks))
              }
              None => None,
          }
      };
      let Some((set_cookie, marks)) = resolved else {
          return capacity_response();
      };
      let overlay = crate::demo::overlay::MarkOverlay::new(&marks);
      let view = crate::demo::overlay::package_view_with_overlay(&state.base_raw, &overlay, mode);
      let markup = render_section(&view[req.root], req.root, None, &state.search_links, mode);
      let mut response = Html(markup.into_string()).into_response();
      if let Some(cookie) = set_cookie {
          response.headers_mut().append(header::SET_COOKIE, cookie);
      }
      response
  }
  ```

  In `events` (currently `src/demo/handlers.rs:272-309`), the change is identical to `index`: the `derive_view` call inside the `if headers.contains_key("last-event-id")` branch becomes the overlay pair.

- [ ] **Step 8: Delete `derive_view`, `marks_for_render`, `marker_order`, and the transient `Mark` struct**

  All three helpers and the transient `Mark` struct introduced in Task 4 are now unused. Delete them from `src/demo/handlers.rs`. Also delete `use crate::raw_view::apply_mark_raw;` from the imports if present; the demo no longer calls it.

  Verify nothing references them:

  ```bash
  rg 'derive_view|marks_for_render|marker_order|struct Mark' src/demo/
  ```

  Expected: zero matches (other than the deletion itself).

- [ ] **Step 9: Run the new tests to verify they pass**

  ```bash
  cargo test --locked --features fixtures --lib demo::overlay::
  cargo test --locked --features fixtures --lib demo::handlers::tests::unmark_
  cargo test --locked --features fixtures --lib demo::handlers::tests::mark_then_unmark_round_trip
  ```

  Expected: all pass.

- [ ] **Step 10: Run the full test suite**

  ```bash
  cargo test --locked --features fixtures
  ```

  Expected: all pass. The existing demo handler tests that exercised mark persistence and `/events` continue to pass because the wire shape is identical and the rendered HTML, by the equivalence test, is byte-equal for the same logical state.

- [ ] **Step 11: Run lint and doc checks (what the pre-commit hook runs)**

  ```bash
  cargo fmt --check
  cargo clippy --locked --all-targets --features fixtures -- -D warnings
  cargo doc --locked --no-deps --features fixtures
  ```

  Expected: all clean.

- [ ] **Step 12: Visually verify the demo via the explore harness**

  Per `CLAUDE.md`'s "Verifying UI changes" workflow. The demo render path changed and the toast Undo button now wires through; both deserve a human glance.

  Check the port is free:

  ```bash
  lsof -iTCP:8919 -sTCP:LISTEN
  ```

  If empty, run:

  ```bash
  cargo run --locked --features fixtures --example explore -- mixed-forest --port 8919
  ```

  Open `http://localhost:8919` and confirm:
  - Marking a folder renders the section correctly (same as before).
  - Clicking the toast's Undo button after a mark removes the mark and the section reverts (this is the new `/unmark` route; previously it 404'd silently).
  - Reset still empties all marks.

  Stop the harness with Ctrl-C when done. The harness tears itself down on signal.

- [ ] **Step 13: Commit**

  ```bash
  git add src/demo.rs src/demo/overlay.rs src/demo/handlers.rs
  git commit -m "feat(demo): render via MarkOverlay, drop derive_view, add /unmark route

  Per-request render cost drops from O((M+1) * F) to O(F * depth). The
  MarkOverlay borrows the session's mark set and answers per-folder
  cleared/exact-marker questions via ancestor walks; package_view_with_overlay
  materializes the overlay against a single clone of the base view and
  forwards to the production package_view, so the renderer body itself is
  shared with production. The Group 1 equivalence test pins byte-for-byte
  parity against the apply_mark_raw replay path across mixed-forest,
  messy-shelf, root-flagged, and pre-marked scenarios in both view modes.

  The demo router gains POST /unmark, which removes one mark from the
  session's set and re-renders the affected section. The production toast's
  Undo button (assets/app.js + src/web/page.rs) stops silently 404-ing in
  the demo.

  Audit item #2 (render half) and the demo /unmark UX inconsistency."
  ```

---

## Self-review checklist

Run these before handing the plan off.

**1. Spec coverage:**
- [x] Critical #1 (mutex poison); Task 3.
- [x] Critical #2 (unbounded derive_view); Task 4 (storage + validation) + Task 5 (overlay render).
- [x] Critical #3 (binary rename); Task 1.
- [x] Critical #4 (--help broken); Task 2.
- [x] Inline /unmark route; Task 5.
- [x] `fixtures` feature placeholder; Task 1.
- [x] Folder-existence validation at /mark and /unmark; Task 4 and Task 5.
- [x] Five-commit migration order; Tasks 1-5 map to commits 1-5 with the same boundaries the spec lays out.
- [x] Test Groups 1-5; Group 1 in Task 5, Group 2 split across Task 4 (/mark) and Task 5 (/unmark), Group 3 in Task 4, Group 4 in Task 5, Group 5 in Task 3.
- [x] No proptest, no new criterion bench (per spec "Not added"); confirmed.

**2. No placeholders:** Searched plan for "TBD", "TODO", "Add appropriate", "Similar to Task"; none present.

**3. Type consistency:**
- `MarkKey = (usize, String, Marker)`: same shape in Task 4 (defined) and Task 5 (consumed).
- `MarkOverlay::new(&'a HashSet<MarkKey>)`: matches `SessionStore::marks` return type from Task 4.
- `package_view_with_overlay(&RawView, &MarkOverlay, ViewMode) -> FlaggedView`: `FlaggedView` is `Vec<RootSection>` from `src/web/render.rs:17`, consumed by `render_section`/`render_view`.
- `folder_exists_in_base(&RawView, usize, &str) -> bool`: same signature in Task 4 (defined) and Task 5 (reused).
- `lock_sessions(&self) -> MutexGuard<'_, SessionStore>`: defined in Task 3, used by all later tasks via the existing handler shape.

---

## Notes for the executor

- Each task's verification depends on the prior tasks being on `main`. If you execute out of order, builds break and tests fail in expected ways.
- The pre-commit hook runs `cargo doc -D warnings`. Any new `pub` item must have a doc comment, or the commit will be rejected. The plan's code blocks include doc comments on every `pub`.
- The plan deliberately keeps `MarkRequest`'s visibility at `pub(crate)`. If a future integration test under `tests/` (outside the crate) needs to drive `/mark`, it can construct the form body as a string and post via `oneshot`; promoting visibility is not required.
- The "Risk" section of the spec calls Commit 5 the load-bearing commit. If the equivalence test in Task 5 fails on a scenario not covered by the test cases, expand `interesting_mark_sets` rather than relaxing the assertion.

