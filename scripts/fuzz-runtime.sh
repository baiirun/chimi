#!/bin/sh
# Runs libFuzzer on one fuzz target of the native runtime
# (compiler/chimi/tests/fuzz/runtime-fuzz.c), starting from its committed
# corpus, under AddressSanitizer and UndefinedBehaviorSanitizer.
#
# Usage, from anywhere inside the repository:
#   scripts/fuzz-runtime.sh TARGET SECONDS [WORK_DIR]
#     TARGET    json, text, decimal or duration
#     SECONDS   how long to fuzz
#     WORK_DIR  where the run keeps its corpus and any crash input
#               (default: a new temporary directory, kept when the run fails)
#
# Needs clang with libFuzzer (on Ubuntu, the libclang-rt-<version>-dev
# package). Exit status: 0 when the run found nothing, 1 when it found a crash
# or a broken invariant (the input is saved as WORK_DIR/crash-*), 2 on bad
# usage or a failed build.
#
# To keep a finding as a regression input once it is fixed, append its corpus
# line and rebless:
#   WORK_DIR/runtime-fuzz TARGET --files WORK_DIR/crash-... >> compiler/chimi/tests/fuzz/corpus/TARGET.txt
#   CHIMI_FUZZ_BLESS=1 cargo test -p chimi --test runtime_fuzz
set -u

usage() { sed -n '6,11p' "$0" | sed 's/^# *//' >&2; exit 2; }
[ $# -ge 2 ] && [ $# -le 3 ] || usage
target=$1
seconds=$2
case "$target" in json | text | decimal | duration) ;; *) usage ;; esac
case "$seconds" in '' | *[!0-9]*) usage ;; esac

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel) || exit 2
crate="$repo/compiler/chimi"
work=${3:-$(mktemp -d "${TMPDIR:-/tmp}/chimi-fuzz-$target.XXXXXX")} || exit 2
mkdir -p "$work/corpus" || exit 2

includes="-I $crate/runtime/native -I $repo/runtime/chimi-fiber/include"
sanitizers="-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer"
# shellcheck disable=SC2086 # the flag lists split on purpose
clang -std=c11 -O1 -g -w $sanitizers $includes "$crate/tests/fuzz/runtime-fuzz.c" \
  -o "$work/runtime-fuzz" -lm || { echo "fuzz-runtime: replay driver build failed" >&2; exit 2; }
# shellcheck disable=SC2086
clang -std=c11 -O1 -g -w -fsanitize=fuzzer $sanitizers -DCHIMI_FUZZ_LIBFUZZER $includes \
  "$crate/tests/fuzz/runtime-fuzz.c" -o "$work/runtime-fuzz-libfuzzer" -lm ||
  { echo "fuzz-runtime: libFuzzer build failed (is libFuzzer installed?)" >&2; exit 2; }

export ASAN_OPTIONS=detect_leaks=0
"$work/runtime-fuzz" --unpack "$crate/tests/fuzz/corpus/$target.txt" "$work/corpus" || exit 2
CHIMI_FUZZ_TARGET=$target "$work/runtime-fuzz-libfuzzer" \
  -max_total_time="$seconds" -max_len=4096 -timeout=10 -print_final_stats=1 \
  -artifact_prefix="$work/" "$work/corpus"
status=$?
if [ "$status" -ne 0 ]; then
  echo "fuzz-runtime: $target found a failure; inputs and corpus are in $work" >&2
  exit 1
fi
[ $# -eq 3 ] || rm -rf "$work"
exit 0
