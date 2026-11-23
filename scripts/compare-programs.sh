#!/bin/sh
# Program parity capture: builds the example programs with the compiler in the
# current checkout, runs them, and either records their outputs or compares
# them with outputs recorded earlier. Use it around a compiler or runtime
# change that must not change what programs print or trace.
#
# Usage, from anywhere inside the repository:
#   scripts/compare-programs.sh record DIR    write outputs into DIR (replacing it)
#   scripts/compare-programs.sh compare DIR   run again and diff against DIR
#
# Typical use: `record /tmp/before` on the old revision, make the change, then
# `compare /tmp/before`. Exit status: 0 when every output matches, 1 on any
# difference (the diff is printed), 2 on bad usage or a failed build.
#
# Runs captured (each run's exit status is recorded in its *.stdout.txt):
#   checkout.*            prototypes/golden/valid/dependencies_and_execution.chimi
#                         with --trace=boundaries; its trace and outcome are the
#                         summarizer's input
#   summary.stdout.txt    programs/chimi-trace-summary reading a file argument
#   summary-stdin.*       the same input on standard input
#   summary-full.*, summary.{trace,outcome}.jsonl
#                         the summarizer under --trace=full, with its own telemetry
#   cp.*, cp-{trace,outcome}.jsonl
#                         programs/concurrency-pipeline under --trace=full
#
# Every run uses a fixed --execution-id. Timing fields (keys ending in _ns, _us,
# _ms, or named elapsed*, duration*, timestamp, time) are replaced with 0 so
# recordings compare across machines and runs, and numeric ids and source
# offsets in telemetry are replaced with 0 so recordings survive compiler
# renumbering.
set -u

usage() { sed -n '7,9p' "$0" | sed 's/^# *//' >&2; exit 2; }
[ $# -eq 2 ] || usage
mode=$1
case "$mode" in record | compare) ;; *) usage ;; esac
case "$2" in /*) directory=$2 ;; *) directory=$PWD/$2 ;; esac

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel) || exit 2
cd "$repo" || exit 2
work=$(mktemp -d "${TMPDIR:-/tmp}/chimi-compare-programs.XXXXXX") || exit 2
trap 'rm -rf "$work"' EXIT
bin="$work/bin"
out="$work/out"
mkdir -p "$bin" "$out"

build() {
  cargo run -q -p chimi -- build "$@" > "$work/build.log" 2>&1 || {
    cat "$work/build.log" >&2
    echo "compare-programs: build failed: $*" >&2
    exit 2
  }
}
build --root programs/chimi-trace-summary app/main -o "$bin/trace-summary"
build prototypes/golden/valid/dependencies_and_execution.chimi -o "$bin/checkout"
build --root programs/concurrency-pipeline app/main -o "$bin/concurrency-pipeline"

# Traced runs name one carrier: a program takes a carrier per core by
# default, and events from several carriers interleave from run to run.
# run NAME COMMAND...: stdout to NAME.stdout.txt followed by "exit STATUS".
run() {
  name=$1; shift
  "$@" > "$out/$name.stdout.txt"
  echo "exit $?" >> "$out/$name.stdout.txt"
}

cd "$out" || exit 2
run checkout "$bin/checkout" --chimi-runner --carriers=1 --trace=boundaries --execution-id=fixed \
  --telemetry-file=checkout.trace.jsonl --outcome-file=checkout.outcome.jsonl --
cat checkout.trace.jsonl checkout.outcome.jsonl > checkout.execution.jsonl
run summary "$bin/trace-summary" checkout.execution.jsonl
run summary-stdin sh -c '"$1" - < checkout.execution.jsonl' sh "$bin/trace-summary"
run summary-full "$bin/trace-summary" --chimi-runner --carriers=1 --trace=full --execution-id=fixed \
  --telemetry-file=summary.trace.jsonl --outcome-file=summary.outcome.jsonl -- checkout.execution.jsonl
run cp "$bin/concurrency-pipeline" --chimi-runner --carriers=1 --trace=full --execution-id=concurrency-pipeline \
  --telemetry-file=cp-trace.jsonl --outcome-file=cp-outcome.jsonl --

for file in *.jsonl *.txt; do
  sed -E 's/("([a-z_]*_(ns|us|ms)|elapsed[a-z_]*|duration[a-z_]*|timestamp|time)":)[0-9.]+/\10/g' \
    "$file" > "$file.normalized" && mv "$file.normalized" "$file"
done
# Trace events also carry compiler-internal numbering (symbol, node, origin,
# semantic, and span ids, and source offsets) that shifts whenever the compiler
# or the standard library gains a declaration. Behaviour is the event sequence,
# operations, attributes, outcomes, and program output, so those numbers are
# replaced with 0 in telemetry. Program stdout is compared exactly.
for file in *.jsonl; do
  sed -E 's/("([a-z_]+_id|source_start|source_end)":)[0-9]+/\10/g' \
    "$file" > "$file.normalized" && mv "$file.normalized" "$file"
done

if [ "$mode" = record ]; then
  rm -rf "$directory"
  mkdir -p "$directory"
  cp "$out"/* "$directory"/
  echo "compare-programs: recorded $(ls "$out" | wc -l | tr -d ' ') outputs in $directory"
  exit 0
fi

[ -d "$directory" ] || { echo "compare-programs: no recording at $directory" >&2; exit 2; }
if diff -ru "$directory" "$out"; then
  echo "compare-programs: all $(ls "$out" | wc -l | tr -d ' ') outputs match $directory"
  exit 0
fi
echo "compare-programs: outputs differ from $directory" >&2
exit 1
