#!/bin/sh
# Runs one CPU-heavy command (cargo build or test, a mutation run, a lab run)
# while holding a lock shared by every worktree of this repository, so agents
# working in parallel take turns on the machine instead of overloading it.
#
# Usage: scripts/heavy.sh <command> [args...]
#   e.g. scripts/heavy.sh cargo test -p chimi --test cli
#
# The command runs at nice 10 with CARGO_BUILD_JOBS and RUST_TEST_THREADS
# defaulting to 3 (override by exporting them). Waiting for the lock prints one
# line naming the holder. A lock whose holder process has exited is taken over.
# Exit status is the command's.

set -u
[ "$#" -gt 0 ] || { echo "usage: scripts/heavy.sh <command> [args...]" >&2; exit 2; }

common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || {
  echo "heavy: not inside the chimi repository" >&2; exit 2; }
lock="$common/chimi-heavy.lock"

announced=no
while ! mkdir "$lock" 2>/dev/null; do
  holder=$(cat "$lock/pid" 2>/dev/null || true)
  if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
    rm -rf "$lock"
    continue
  fi
  if [ "$announced" = no ]; then
    echo "heavy: waiting for pid ${holder:-?}: $(cat "$lock/command" 2>/dev/null || echo unknown)" >&2
    announced=yes
  fi
  sleep 2
done
echo $$ > "$lock/pid"
echo "$*" > "$lock/command"
trap 'rm -rf "$lock"' EXIT INT TERM HUP

CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-3} RUST_TEST_THREADS=${RUST_TEST_THREADS:-3} \
  nice -n 10 "$@"
