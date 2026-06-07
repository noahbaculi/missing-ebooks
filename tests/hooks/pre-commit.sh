#!/bin/sh
# Exercise the pre-commit hook's control flow with a stubbed cargo, so the test
# stays fast and deterministic. Real cargo fmt and clippy behavior is covered by
# CI; here we only check that the hook skips non-Rust commits, runs both checks
# in order, and blocks the commit when either one fails.
set -eu

REPO_ROOT=$(git rev-parse --show-toplevel)
HOOK="$REPO_ROOT/.githooks/pre-commit"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# A fake cargo on PATH: it records each call to $CARGO_LOG and exits with the
# code we choose per check, so we can simulate fmt or clippy failing.
STUB="$WORK/bin"
mkdir -p "$STUB"
cat > "$STUB/cargo" <<'STUB_EOF'
#!/bin/sh
echo "$*" >> "$CARGO_LOG"
case "$1" in
  fmt) exit "${CARGO_FMT_EXIT:-0}" ;;
  clippy) exit "${CARGO_CLIPPY_EXIT:-0}" ;;
esac
exit 0
STUB_EOF
chmod +x "$STUB/cargo"

export CARGO_LOG="$WORK/cargo.log"
export CARGO_FMT_EXIT=0
export CARGO_CLIPPY_EXIT=0
fail=0

# Build a fresh repo and stage the named (empty) files. Usage: stage_case f1 [f2...]
stage_case() {
  rm -rf "$WORK/repo"
  mkdir -p "$WORK/repo"
  : > "$CARGO_LOG"
  git init -q "$WORK/repo"
  git -C "$WORK/repo" config user.email test@example.com
  git -C "$WORK/repo" config user.name test
  for f in "$@"; do
    mkdir -p "$WORK/repo/$(dirname "$f")"
    : > "$WORK/repo/$f"
    git -C "$WORK/repo" add "$f"
  done
}

# Run the hook in the staged repo and print its exit code (and nothing else).
hook_exit() {
  rc=0
  ( cd "$WORK/repo" && PATH="$STUB:$PATH" sh "$HOOK" >/dev/null 2>&1 ) || rc=$?
  printf '%s' "$rc"
}

expect_exit() { # label expected actual
  if [ "$2" -eq "$3" ]; then
    echo "ok: $1"
  else
    echo "FAIL: $1 (expected exit $2, got $3)" >&2
    fail=1
  fi
}
expect_log_has() { # label needle
  if grep -qF "$2" "$CARGO_LOG"; then
    echo "ok: $1"
  else
    echo "FAIL: $1 (missing '$2' in cargo calls)" >&2
    fail=1
  fi
}
expect_log_missing() { # label needle
  if grep -qF "$2" "$CARGO_LOG"; then
    echo "FAIL: $1 (unexpected '$2' in cargo calls)" >&2
    fail=1
  else
    echo "ok: $1"
  fi
}
expect_log_empty() { # label
  if [ -s "$CARGO_LOG" ]; then
    echo "FAIL: $1 (cargo ran but should have been skipped)" >&2
    fail=1
  else
    echo "ok: $1"
  fi
}

# 1. Only a Markdown file staged: hook skips, cargo never runs.
stage_case notes.md
CARGO_FMT_EXIT=0; CARGO_CLIPPY_EXIT=0
expect_exit "skip: markdown-only exits 0" 0 "$(hook_exit)"
expect_log_empty "skip: cargo not invoked"

# 2. Clean Rust file: both checks run and pass.
stage_case src/main.rs
CARGO_FMT_EXIT=0; CARGO_CLIPPY_EXIT=0
expect_exit "clean: passing checks exit 0" 0 "$(hook_exit)"
expect_log_has "clean: ran fmt" "fmt --check"
expect_log_has "clean: ran clippy" "clippy"

# 3. fmt fails: hook blocks and never reaches clippy.
stage_case src/main.rs
CARGO_FMT_EXIT=1; CARGO_CLIPPY_EXIT=0
expect_exit "fmt-fail: blocks commit" 1 "$(hook_exit)"
expect_log_has "fmt-fail: ran fmt" "fmt --check"
expect_log_missing "fmt-fail: skipped clippy" "clippy"

# 4. clippy fails: hook blocks after fmt passes.
stage_case src/main.rs
CARGO_FMT_EXIT=0; CARGO_CLIPPY_EXIT=1
expect_exit "clippy-fail: blocks commit" 1 "$(hook_exit)"
expect_log_has "clippy-fail: ran clippy" "clippy"

if [ "$fail" -ne 0 ]; then
  echo "hook tests FAILED" >&2
  exit 1
fi
echo "hook tests passed"
