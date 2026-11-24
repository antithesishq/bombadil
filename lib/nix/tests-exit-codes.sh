#!/usr/bin/env bash
# Checks the exit codes documented in the CLI reference.
#
# Usage: tests-exit-codes.sh <bombadil> [terminal] [browser]
set -u

bombadil=$(realpath "${1:?usage: tests-exit-codes.sh <bombadil> [terminal] [browser]}")
shift
if [ $# -eq 0 ]; then
  set -- terminal browser
fi

cd "$(mktemp -d)"

cat > terminal-violation.ts <<'EOF'
import { always } from "@antithesishq/bombadil";
import { actions } from "@antithesishq/bombadil/terminal";
export const violated = always(() => false);
export const typing = actions(() => [{ TypeText: { CharSet: [{ Literal: "x" }] } }]);
EOF

cat > terminal-pass.ts <<'EOF'
import { always } from "@antithesishq/bombadil";
import { actions } from "@antithesishq/bombadil/terminal";
export const holds = always(() => true);
export const typing = actions(() => [{ TypeText: { CharSet: [{ Literal: "x" }] } }]);
EOF

cat > browser-violation.ts <<'EOF'
import { always } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";
export const violated = always(() => false);
EOF

cat > browser-pass.ts <<'EOF'
import { always } from "@antithesishq/bombadil";
export { clicks } from "@antithesishq/bombadil/browser/defaults/actions";
export const holds = always(() => true);
EOF

cat > index.html <<'EOF'
<!doctype html>
<html><head><title>Exit codes</title></head><body><button>OK</button></body></html>
EOF

failures=0

# expect <code> <name> <bombadil args...>
expect() {
  local expected=$1 name=$2 actual
  shift 2
  timeout 120 "$bombadil" "$@" > "$name.log" 2>&1
  actual=$?
  if [ "$actual" -eq "$expected" ]; then
    echo "ok    $name: exit $actual"
  else
    echo "FAIL  $name: exit $actual, expected $expected"
    echo "      bombadil $*"
    sed 's/^/      | /' "$name.log"
    failures=$((failures + 1))
  fi
}

terminal() {
  expect 2 terminal-test-violation \
    terminal test --specification terminal-violation.ts \
    --time-limit 3s --output-path terminal-test-violation -- cat
  expect 2 terminal-test-violation-exit-early \
    terminal test --specification terminal-violation.ts --exit-on-violation \
    --time-limit 30s --output-path terminal-test-violation-exit-early -- cat
  expect 0 terminal-test-pass \
    terminal test --specification terminal-pass.ts \
    --time-limit 3s --output-path terminal-test-pass -- cat
  expect 1 terminal-test-error \
    terminal test --specification missing.ts \
    --time-limit 3s --output-path terminal-test-error -- cat

  expect 2 terminal-fuzz-violation \
    terminal fuzz --specification terminal-violation.ts \
    --time-limit-run 2s --time-limit-fuzz 5s \
    --output-path terminal-fuzz-violation -- cat
  expect 2 terminal-fuzz-violation-exit-early \
    terminal fuzz --specification terminal-violation.ts --exit-on-violation \
    --time-limit-run 30s --time-limit-fuzz 30s \
    --output-path terminal-fuzz-violation-exit-early -- cat
  expect 0 terminal-fuzz-pass \
    terminal fuzz --specification terminal-pass.ts \
    --time-limit-run 2s --time-limit-fuzz 5s \
    --output-path terminal-fuzz-pass -- cat
  expect 1 terminal-fuzz-error \
    terminal fuzz --specification missing.ts \
    --time-limit-run 2s --time-limit-fuzz 5s \
    --output-path terminal-fuzz-error -- cat
  expect 1 terminal-fuzz-runs-failed \
    terminal fuzz --specification terminal-pass.ts \
    --time-limit-run 2s --time-limit-fuzz 5s \
    --output-path terminal-fuzz-runs-failed -- ./does-not-exist
}

browser() {
  local origin="file://$PWD/index.html"

  expect 2 browser-test-violation \
    browser test --headless --no-sandbox \
    --time-limit 5s --output-path browser-test-violation.bin \
    "$origin" browser-violation.ts
  expect 2 browser-test-violation-exit-early \
    browser test --headless --no-sandbox --exit-on-violation \
    --time-limit 60s --output-path browser-test-violation-exit-early.bin \
    "$origin" browser-violation.ts
  expect 0 browser-test-pass \
    browser test --headless --no-sandbox \
    --time-limit 5s --output-path browser-test-pass.bin \
    "$origin" browser-pass.ts
  expect 1 browser-test-error \
    browser test --headless --no-sandbox \
    --time-limit 5s --output-path browser-test-error.bin \
    "$origin" missing.ts

  expect 2 browser-fuzz-violation \
    browser fuzz --headless --no-sandbox \
    --time-limit-run 5s --time-limit-fuzz 10s \
    --output-path browser-fuzz-violation.bin \
    "$origin" browser-violation.ts
  expect 2 browser-fuzz-violation-exit-early \
    browser fuzz --headless --no-sandbox --exit-on-violation \
    --time-limit-run 60s --time-limit-fuzz 60s \
    --output-path browser-fuzz-violation-exit-early.bin \
    "$origin" browser-violation.ts
  expect 0 browser-fuzz-pass \
    browser fuzz --headless --no-sandbox \
    --time-limit-run 5s --time-limit-fuzz 10s \
    --output-path browser-fuzz-pass.bin \
    "$origin" browser-pass.ts
  expect 1 browser-fuzz-error \
    browser fuzz --headless --no-sandbox \
    --time-limit-run 5s --time-limit-fuzz 10s \
    --output-path browser-fuzz-error.bin \
    "$origin" missing.ts
}

for suite in "$@"; do
  case "$suite" in
    terminal) terminal ;;
    browser) browser ;;
    *)
      echo "unknown suite: $suite" >&2
      exit 1
      ;;
  esac
done

if [ "$failures" -gt 0 ]; then
  echo "$failures exit code check(s) failed"
  exit 1
fi
