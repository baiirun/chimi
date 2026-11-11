#!/bin/sh
# Regenerates compiler/chimi/tests/fixtures/native-heap-results.txt: the per-message
# cost of the native heap's fragments and the memory curve of the concurrency
# pipeline as its job count grows. Run from the repository root:
#
#   sh compiler/chimi/tests/fixtures/native-heap-measure.sh > compiler/chimi/tests/fixtures/native-heap-results.txt
#
# Byte counts are exact and repeatable. Times depend on the machine and its load.
set -e
ROOT=$(pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "# Native heap measurements"
echo "# command: sh compiler/chimi/tests/fixtures/native-heap-measure.sh"
echo "# commit: $(git rev-parse --short HEAD)"
echo "# machine: $(uname -sm), $(cc --version | head -1)"
echo

cc -std=c11 -O2 -I compiler/chimi/runtime/native -I runtime/chimi-fiber/include \
  compiler/chimi/tests/fixtures/native-heap.c -o "$WORK/native-heap"
echo "## Int messages: footprint of 1,000 queued; cost of one (sender copy and pass, adoption, receiver pass; no fiber switch)"
"$WORK/native-heap" measure | grep '^measure'
echo

echo "## Concurrency pipeline memory by job count"
echo "# jobs value unscoped_allocated_bytes scoped_retained_bytes scoped_peak_live_bytes peak_footprint_bytes peak_fragment_footprint_bytes scoped_collections scoped_full_collections"
cargo build -q -p chimi
for JOBS in 50 500 5000 50000; do
  sed "s/@JOBS@/$JOBS/" > "$WORK/pipeline.chimi" <<'EOF'
import std/channel as Channel
import std/channel.{Completion, Receiver, Receive, Sender}
import std/task as Task
fn worker(jobs: Receiver<Int>, results: Sender<Int>, completion: Completion<Int>) -> Int {
  loop processed = 0 {
    let next = Channel.receive(jobs)
    case next {
      Receive.Received(value) -> {
        Channel.send(results, value + 10)
        processed + 1
      }
      Receive.Drained -> {
        Channel.close(completion)
        break processed
      }
    }
  }
}
pub fn main() -> Int {
  let jobs = Channel.bounded(1)
  let results = Channel.bounded(1)
  let job_sender = jobs.sender
  let job_receiver = jobs.receiver
  let job_completion = jobs.completion
  let result_sender = results.sender
  let result_receiver = results.receiver
  let result_completion = results.completion
  let child = Task.start(fn() -> Int { worker(job_receiver, result_sender, result_completion) })
  let total = loop state = { next: 0, sum: 0 } {
    case state.next {
      @JOBS@ -> break state.sum
      _ -> {
        Channel.send(job_sender, state.next)
        let reply = Channel.receive(result_receiver)
        case reply {
          Receive.Received(value) -> { next: state.next + 1, sum: state.sum + value }
          Receive.Drained -> break state.sum
        }
      }
    }
  }
  Channel.close(job_completion)
  total + Task.result(child)
}
EOF
  (cd "$WORK" && "$ROOT/target/debug/chimi" build pipeline.chimi -o pipeline >/dev/null)
  rm -f "$WORK/outcome.json" "$WORK/memory.json"
  "$WORK/pipeline" --chimi-runner --trace=off --outcome-file="$WORK/outcome.json" \
    --memory-report-file="$WORK/memory.json" -- >/dev/null
  python3 - "$JOBS" "$WORK/outcome.json" "$WORK/memory.json" <<'EOF'
import json, sys
outcome = json.loads(open(sys.argv[2]).read())
report = json.loads(open(sys.argv[3]).read())
fields = ["unscoped_allocated_bytes", "scoped_retained_bytes", "scoped_peak_live_bytes",
          "peak_footprint_bytes", "peak_fragment_footprint_bytes", "scoped_collections",
          "scoped_full_collections"]
print(sys.argv[1], outcome.get("value"), *(report[field] for field in fields))
EOF
done
