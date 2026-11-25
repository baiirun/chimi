#!/bin/bash
# Runs one shard of the workspace's tests, for the CI job that runs them with
# sanitized native programs (.github/workflows/ci.yml, `sanitizers`). Under
# the sanitizers the whole suite takes about an hour and a half, so it runs
# in six shards of 20 to 30 minutes each:
#   cli          the cli tests
#   process-lab  the process lab's scenarios
#   rest-N       every other test file of compiler/chimi, dealt out by name
#                in turn to rest-1 to rest-4; rest-1 also runs the library's
#                unit tests, and rest-4 the binaries' unit tests, the doc
#                tests and the other workspace packages
# A test file added later lands in a rest shard without a change here.
# Usage, from the repository root: scripts/sanitizer-shard.sh SHARD
set -euo pipefail

others() {
  local shard=$1 index=0 name
  for file in compiler/chimi/tests/*.rs; do
    name=$(basename "$file" .rs)
    [ "$name" = cli ] || [ "$name" = process_lab ] && continue
    [ $((index % 4)) -eq "$shard" ] && printf -- '--test %s ' "$name"
    index=$((index + 1))
  done
}

case "${1:-}" in
  cli) cargo test -p chimi --quiet --no-fail-fast --test cli ;;
  process-lab) cargo test -p chimi --quiet --no-fail-fast --test process_lab ;;
  # shellcheck disable=SC2046 # one word per flag
  rest-1) cargo test -p chimi --quiet --no-fail-fast --lib $(others 0) ;;
  # shellcheck disable=SC2046
  rest-2) cargo test -p chimi --quiet --no-fail-fast $(others 1) ;;
  # shellcheck disable=SC2046
  rest-3) cargo test -p chimi --quiet --no-fail-fast $(others 2) ;;
  rest-4)
    # Each runs even when one before it failed; the shard fails if any did.
    status=0
    # shellcheck disable=SC2046
    cargo test -p chimi --quiet --no-fail-fast --bins $(others 3) || status=1
    cargo test -p chimi --quiet --doc || status=1
    cargo test -p chimi-lsp -p chimi-fiber --quiet --no-fail-fast || status=1
    exit "$status"
    ;;
  *) echo "usage: scripts/sanitizer-shard.sh cli|process-lab|rest-1|rest-2|rest-3|rest-4" >&2; exit 2 ;;
esac
