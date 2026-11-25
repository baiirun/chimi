#!/bin/bash
# Runs one shard of the workspace's tests, for the CI job that runs them with
# sanitized native programs (.github/workflows/ci.yml, `sanitizers`). Under
# the sanitizers the whole suite takes about an hour and a half, so it runs
# in four shards of a quarter to a third of that each:
#   cli          the cli tests
#   process-lab  the process lab's scenarios
#   rest-1       every other test file of compiler/chimi, the even ones in
#                name order, and the library's unit tests
#   rest-2       the odd ones, the binaries' unit tests, the doc tests and
#                the other workspace packages
# A test file added later lands in rest-1 or rest-2 without a change here.
# Usage, from the repository root: scripts/sanitizer-shard.sh SHARD
set -euo pipefail

others() {
  local parity=$1 index=0 name
  for file in compiler/chimi/tests/*.rs; do
    name=$(basename "$file" .rs)
    [ "$name" = cli ] || [ "$name" = process_lab ] && continue
    [ $((index % 2)) -eq "$parity" ] && printf -- '--test %s ' "$name"
    index=$((index + 1))
  done
}

case "${1:-}" in
  cli) cargo test -p chimi --quiet --no-fail-fast --test cli ;;
  process-lab) cargo test -p chimi --quiet --no-fail-fast --test process_lab ;;
  # shellcheck disable=SC2046 # one word per flag
  rest-1) cargo test -p chimi --quiet --no-fail-fast --lib $(others 0) ;;
  rest-2)
    # Each runs even when one before it failed; the shard fails if any did.
    status=0
    # shellcheck disable=SC2046
    cargo test -p chimi --quiet --no-fail-fast --bins $(others 1) || status=1
    cargo test -p chimi --quiet --doc || status=1
    cargo test -p chimi-lsp -p chimi-fiber --quiet --no-fail-fast || status=1
    exit "$status"
    ;;
  *) echo "usage: scripts/sanitizer-shard.sh cli|process-lab|rest-1|rest-2" >&2; exit 2 ;;
esac
