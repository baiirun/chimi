#!/usr/bin/env bash
# Checks that the editor grammar in editors/tree-sitter-chimi keeps up with the
# compiler, which is the authority on Chimi syntax:
# 1. the committed parser in src/ matches grammar.js;
# 2. the grammar's own corpus tests pass;
# 3. every .chimi file in the repository that the compiler parses (judged by
#    `chimi format`, which reports syntax errors and checks nothing else) also
#    parses in the grammar with no error or missing node. Files the compiler
#    rejects, such as invalid-syntax fixtures, are skipped.
#
# Usage: scripts/check-grammar.sh
# Environment: CHIMI (default target/debug/chimi), TREE_SITTER (default
# tree-sitter on PATH; it must be the version named in src/parser.c, or the
# regenerated parser differs).
set -euo pipefail

root=$(git rev-parse --show-toplevel)
chimi=${CHIMI:-$root/target/debug/chimi}
tree_sitter=${TREE_SITTER:-tree-sitter}
grammar=$root/editors/tree-sitter-chimi

cd "$grammar"
"$tree_sitter" generate
if ! git diff --quiet -- src; then
  git diff --stat -- src
  echo "editors/tree-sitter-chimi/src is out of date: run \`tree-sitter generate\` there and commit src/." >&2
  exit 1
fi
"$tree_sitter" test

valid=$(mktemp)
skipped=0
while IFS= read -r -d '' file; do
  if "$chimi" format "$root/$file" >/dev/null 2>&1; then
    echo "$root/$file" >>"$valid"
  else
    skipped=$((skipped + 1))
  fi
done < <(git -C "$root" ls-files -z -- '*.chimi')

count=$(wc -l <"$valid" | tr -d ' ')
if [ "$count" -eq 0 ]; then
  echo "no .chimi file passed \`chimi format\`; is $chimi built?" >&2
  exit 1
fi
if ! "$tree_sitter" parse --quiet --paths "$valid"; then
  echo "the grammar rejects files the compiler parses (listed above); update editors/tree-sitter-chimi/grammar.js." >&2
  exit 1
fi
echo "grammar parses all $count .chimi files the compiler accepts ($skipped with syntax errors skipped)"
