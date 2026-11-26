#!/bin/bash
# Builds the native heap fixture with the heap check on and without
# AddressSanitizer, and runs it: the check alone must catch a collector bug,
# as it will have to once the collector is Rust, which the sanitizer does not
# instrument. Run by heap_verify_mutations.py from compiler/chimi, with
# CARGO_TARGET_DIR set.
set -euo pipefail
cargo build --quiet -p chimi
archive=$(ls -t "$CARGO_TARGET_DIR"/debug/build/chimi-*/out/libchimi_runtime_standalone.a | head -1)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cc -std=c11 -O1 -g -DCHIMI_HEAP_VERIFY -I runtime/native -I ../../runtime/chimi-fiber/include \
  tests/fixtures/native-heap.c "$archive" -o "$work/native-heap" -lm
"$work/native-heap"
