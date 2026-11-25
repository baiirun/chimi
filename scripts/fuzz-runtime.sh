#!/bin/sh
# Runs libFuzzer on one fuzz target of the native runtime
# (compiler/chimi/tests/fuzz/runtime-fuzz.c) or of the binding packages
# (binding-fuzz.c), starting from its committed corpus, under
# AddressSanitizer and UndefinedBehaviorSanitizer.
#
# Usage, from anywhere inside the repository:
#   scripts/fuzz-runtime.sh TARGET SECONDS [WORK_DIR]
#     TARGET    json, text, decimal, duration (the runtime), or
#               tls-client, tls-server, tls-records, tls-roots, scram, sqlite
#     SECONDS   how long to fuzz
#     WORK_DIR  where the run keeps its corpus and any crash input
#               (default: a new temporary directory, kept when the run fails)
#
# Needs clang with libFuzzer (on Ubuntu, the libclang-rt-<version>-dev
# package) and cargo: the parsers that moved to Rust (CHI-305) are built from
# runtime/chimi-runtime with libFuzzer's coverage instrumentation, so the
# fuzzer also steers by the Rust code's branches. Exit status: 0 when the run found nothing, 1 when it found a crash
# or a broken invariant (the input is saved as WORK_DIR/crash-*), 2 on bad
# usage or a failed build.
#
# To keep a finding as a regression input once it is fixed, append its corpus
# line and rebless:
#   WORK_DIR/HARNESS TARGET --files WORK_DIR/crash-... >> compiler/chimi/tests/fuzz/corpus/TARGET.txt
#   CHIMI_FUZZ_BLESS=1 cargo test -p chimi --test runtime_fuzz
# where HARNESS is runtime-fuzz or binding-fuzz.
set -u

usage() { sed -n '7,13p' "$0" | sed 's/^# *//' >&2; exit 2; }
[ $# -ge 2 ] && [ $# -le 3 ] || usage
target=$1
seconds=$2
case "$target" in
  json | text | decimal | duration) harness=runtime-fuzz ;;
  tls-client | tls-server | tls-records | tls-roots | scram | sqlite) harness=binding-fuzz ;;
  *) usage ;;
esac
case "$seconds" in '' | *[!0-9]*) usage ;; esac

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel) || exit 2
crate="$repo/compiler/chimi"
bindings="$crate/runtime/native/bindings"
work=${3:-$(mktemp -d "${TMPDIR:-/tmp}/chimi-fuzz-$target.XXXXXX")} || exit 2
mkdir -p "$work/corpus" || exit 2

sanitizers="-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer"
if [ "$harness" = runtime-fuzz ]; then
  includes="-I $crate/runtime/native -I $repo/runtime/chimi-fiber/include"
  # The Rust runtime archive, as build.rs makes it (only its own object, so the
  # C toolchain's helpers serve runtime.c), instrumented as cargo-fuzz does.
  coverage="-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4"
  coverage="$coverage -Cllvm-args=-sanitizer-coverage-inline-8bit-counters"
  coverage="$coverage -Cllvm-args=-sanitizer-coverage-pc-table"
  coverage="$coverage -Cllvm-args=-sanitizer-coverage-trace-compares"
  RUSTFLAGS=$coverage cargo rustc --quiet --manifest-path "$repo/Cargo.toml" \
    -p chimi-runtime-standalone --lib --profile runtime-standalone --crate-type staticlib \
    --target-dir "$work/rust" || { echo "fuzz-runtime: Rust runtime build failed" >&2; exit 2; }
  mkdir -p "$work/rust/objects" || exit 2
  member=$(cd "$work/rust/objects" && ar t ../runtime-standalone/libchimi_runtime_standalone.a |
    grep '^chimi_runtime_standalone-') || exit 2
  (cd "$work/rust/objects" && rm -f libchimi_runtime_standalone.a &&
    ar x ../runtime-standalone/libchimi_runtime_standalone.a "$member" &&
    ar rcs libchimi_runtime_standalone.a "$member") || exit 2
  rust_runtime="$work/rust/objects/libchimi_runtime_standalone.a"
  archives=$rust_runtime
else
  # Each package's own flags, as BINDING_PACKAGE_FLAGS in
  # compiler/chimi/src/backend/runtime_archive_key.rs gives them (a test
  # checks this list holds them), with its -I directories made absolute.
  tls_flags="-I$bindings/tls/mbedtls/include -I$bindings/tls/mbedtls/library
    -I$bindings/tls/mbedtls/tf-psa-crypto/include
    -I$bindings/tls/mbedtls/tf-psa-crypto/drivers/builtin/include
    -I$bindings/tls/mbedtls/tf-psa-crypto/drivers/builtin/src
    -I$bindings/tls/mbedtls/tf-psa-crypto/core -I$bindings/tls/mbedtls/tf-psa-crypto/dispatch
    -I$bindings/tls/mbedtls/tf-psa-crypto/utilities
    -I$bindings/tls/mbedtls/tf-psa-crypto/platform -I$bindings/tls/mbedtls/tf-psa-crypto/extras"
  sqlite_flags="-D_DEFAULT_SOURCE -D_DARWIN_C_SOURCE -DSQLITE_THREADSAFE=1 -DSQLITE_DQS=0
    -DSQLITE_DEFAULT_MEMSTATUS=0 -DSQLITE_LIKE_DOESNT_MATCH_BLOBS -DSQLITE_OMIT_DEPRECATED
    -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_OMIT_SHARED_CACHE -DSQLITE_USE_ALLOCA"
  includes="-I $crate/runtime/native -I $bindings/tls -I $bindings/sqlite -I $bindings/sqlite/sqlite
    $tls_flags $sqlite_flags"
  # The packages' code, with libFuzzer's coverage, compiled several files
  # at once; the replay build gets the same objects.
  for package in tls sqlite; do
    mkdir -p "$work/objects/$package" || exit 2
    eval "flags=\$${package}_flags"
    # shellcheck disable=SC2154 # flags is set by the eval
    (cd "$bindings/$package" && find . -name '*.c' | sort) |
      while read -r source; do
        object="$work/objects/$package/$(printf '%s' "$source" | tr '/' '_').o"
        printf '%s %s\n' "$bindings/$package/$source" "$object"
      done |
      xargs -P "$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)" -n 2 sh -c '
        # shellcheck disable=SC2086
        clang -std=c11 -O1 -g -w $0 -I "$1" -I "$2" $3 -c "$4" -o "$5"
      ' "$sanitizers -fsanitize=fuzzer-no-link" "$bindings/$package" "$crate/runtime/native" "$flags" ||
      { echo "fuzz-runtime: building the $package package failed" >&2; exit 2; }
  done
  archives=$(find "$work/objects" -name '*.o' | sort | tr '\n' ' ')
  export CHIMI_FUZZ_TLS_FIXTURES="$crate/tests/fixtures/tls"
fi

# shellcheck disable=SC2086 # the flag lists split on purpose
clang -std=c11 -O1 -g -w $sanitizers $includes "$crate/tests/fuzz/$harness.c" $archives \
  -o "$work/$harness" -lpthread -lm || { echo "fuzz-runtime: replay driver build failed" >&2; exit 2; }
# shellcheck disable=SC2086
clang -std=c11 -O1 -g -w -fsanitize=fuzzer $sanitizers -DCHIMI_FUZZ_LIBFUZZER $includes \
  "$crate/tests/fuzz/$harness.c" $archives -o "$work/$harness-libfuzzer" -lpthread -lm ||
  { echo "fuzz-runtime: libFuzzer build failed (is libFuzzer installed?)" >&2; exit 2; }

export ASAN_OPTIONS=detect_leaks=0
"$work/$harness" --unpack "$crate/tests/fuzz/corpus/$target.txt" "$work/corpus" || exit 2
CHIMI_FUZZ_TARGET=$target "$work/$harness-libfuzzer" \
  -max_total_time="$seconds" -max_len=4096 -timeout=10 -print_final_stats=1 \
  -artifact_prefix="$work/" "$work/corpus"
status=$?
if [ "$status" -ne 0 ]; then
  echo "fuzz-runtime: $target found a failure; inputs and corpus are in $work" >&2
  exit 1
fi
[ $# -eq 3 ] || rm -rf "$work"
exit 0
