#!/bin/bash
# Builds the native heap fixture under AddressSanitizer and UBSan against the
# runtime archive of this checkout, and runs it: the C harness drives region
# passes directly, so it catches a collector bug that frees, keeps or
# rescans the wrong allocations. Run by collector_mutations.py from
# compiler/chimi, with CARGO_TARGET_DIR set.
set -euo pipefail
cargo build --quiet -p chimi
archive=$(ls -t "$CARGO_TARGET_DIR"/debug/build/chimi-*/out/libchimi_runtime_standalone.a | head -1)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cc -std=c11 -O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer -I runtime/native \
  -I ../../runtime/chimi-fiber/include tests/fixtures/native-heap.c "$archive" -o "$work/native-heap" -lm -lpthread
ASAN_OPTIONS=detect_leaks=0 UBSAN_OPTIONS=halt_on_error=1 "$work/native-heap"
