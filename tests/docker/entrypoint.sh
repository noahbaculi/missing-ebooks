#!/bin/sh
# Exercise the container entrypoint without Docker. A stubbed su-exec on PATH
# records the command it was handed, so we can assert on PUID/PGID defaulting,
# config auto-detection, and argument passthrough. Real privilege-dropping is
# covered by the image smoke test in CI.
set -eu

REPO_ROOT=$(git rev-parse --show-toplevel)
ENTRYPOINT="$REPO_ROOT/docker/entrypoint.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# A fake su-exec on PATH: print what it was asked to run, then stop (so the
# real server binary is never executed).
STUB="$WORK/bin"
mkdir -p "$STUB"
cat > "$STUB/su-exec" <<'STUB_EOF'
#!/bin/sh
echo "su-exec args: $*"
STUB_EOF
chmod +x "$STUB/su-exec"

fail=0
# run <env assignments...> -- captures the entrypoint's stdout
run_entrypoint() {
  PATH="$STUB:$PATH" sh "$ENTRYPOINT" "$@"
}

expect_contains() { # label haystack needle
  case "$2" in
    *"$3"*) echo "ok: $1" ;;
    *) echo "FAIL: $1" >&2; echo "  expected to contain: $3" >&2; echo "  got: $2" >&2; fail=1 ;;
  esac
}

# Case 1: PUID/PGID default to 1000:1000, no extra args.
out=$(run_entrypoint)
expect_contains "default PUID/PGID" "$out" "su-exec args: 1000:1000 missing-ebooks"

# Case 2: explicit PUID/PGID are honored.
out=$(PUID=1500 PGID=1600 run_entrypoint)
expect_contains "explicit PUID/PGID" "$out" "su-exec args: 1500:1600 missing-ebooks"

# Case 3: extra args pass through to the server.
out=$(run_entrypoint --print-config)
expect_contains "arg passthrough" "$out" "missing-ebooks --print-config"

# Case 4: a present config file is auto-added as --config.
CFG="$WORK/config.toml"
echo 'library_roots = ["/tmp/lib"]' > "$CFG"
out=$(CONFIG_FILE="$CFG" run_entrypoint)
expect_contains "config auto-detect" "$out" "missing-ebooks --config $CFG"

# Case 5: an explicit --config is not duplicated by auto-detection.
out=$(CONFIG_FILE="$CFG" run_entrypoint --config /other.toml)
expect_contains "explicit --config wins" "$out" "missing-ebooks --config /other.toml"
case "$out" in
  *"--config $CFG"*) echo "FAIL: auto-config should not be added when --config is explicit" >&2; fail=1 ;;
  *) echo "ok: no duplicate --config" ;;
esac

if [ "$fail" -ne 0 ]; then
  echo "entrypoint tests FAILED" >&2
  exit 1
fi
echo "entrypoint tests passed"
