#!/bin/sh
# Builds and tests exactly what is committed at a revision, isolated from any
# uncommitted work in the shared checkout. Exit status is the test result.
# Usage: scripts/check-head.sh [revision] [scratch-dir]
set -eu
revision="${1:-HEAD}"
scratch="${2:-${TMPDIR:-/tmp}/chimi-check-head}"
repo="$(git rev-parse --show-toplevel)"
commit="$(git -C "$repo" rev-parse --verify "$revision^{commit}")"
tree="$scratch/tree"
rm -rf "$tree"
mkdir -p "$tree"
git -C "$repo" archive "$commit" | tar -x -C "$tree"
export CARGO_TARGET_DIR="$scratch/target"
cd "$tree"
echo "check-head: testing $commit"
cargo test --workspace --quiet
echo "check-head: $commit is green"
