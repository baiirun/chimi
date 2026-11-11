#!/bin/sh
# Integrates one agent worktree branch into main, stopping at the first failure.
#
# Steps: refuse during a merge; rebase the branch onto main in its worktree
# (stops on conflicts for a human or coordinator to resolve, then rerun);
# record program outputs on main; run the workspace tests on the rebased
# branch; fast-forward main; compare program outputs; test the exact commit
# from a clean export; push; remove the worktree once git cherry shows it fully
# merged and it has no uncommitted work.
#
# Usage: scripts/integrate-branch.sh <branch> [scratch-dir]
set -eu
branch="$1"
scratch="${2:-${TMPDIR:-/tmp}/chimi-integrate}"
repo="$(git rev-parse --show-toplevel)"
cd "$repo"

if [ -f "$(git rev-parse --git-dir)/MERGE_HEAD" ]; then
  echo "integrate: a merge is in progress in the main checkout" >&2
  exit 1
fi

worktree="$(git worktree list --porcelain | awk -v b="refs/heads/$branch" '
  /^worktree / { path = substr($0, 10) }
  $0 == "branch " b { print path }')"
if [ -n "$worktree" ]; then
  if [ -n "$(git -C "$worktree" status --porcelain --untracked-files=no)" ]; then
    echo "integrate: $worktree has uncommitted changes" >&2
    exit 1
  fi
  git -C "$worktree" rebase main
fi

mkdir -p "$scratch"
baseline="$scratch/parity-main"
rm -rf "$baseline"
scripts/compare-programs.sh record "$baseline" > "$scratch/parity-record.log" 2>&1

test_dir="${worktree:-$repo}"
echo "integrate: testing $branch in $test_dir"
(cd "$test_dir" && CARGO_TARGET_DIR="$scratch/target" cargo test --workspace --quiet) \
  > "$scratch/tests.log" 2>&1 || {
  echo "integrate: workspace tests failed; see $scratch/tests.log" >&2
  exit 1
}

git merge --ff-only --quiet "$branch"
revision="$(git rev-parse HEAD)"
scripts/compare-programs.sh compare "$baseline"
scripts/check-head.sh "$revision" "$scratch/check-head" > "$scratch/check-head.log" 2>&1 || {
  echo "integrate: clean-export check failed; see $scratch/check-head.log" >&2
  exit 1
}
git push origin "${revision}:refs/heads/main"

if [ -n "$worktree" ] && [ -z "$(git cherry main "$branch")" ]; then
  git worktree remove -f -f "$worktree"
  git branch -D "$branch"
fi
echo "integrate: $branch is on main at $revision and pushed"
