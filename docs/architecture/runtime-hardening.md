# Runtime hardening: sanitizers and fuzzing

The native runtime is C today (`compiler/chimi/runtime/native/runtime.c` and the
binding packages under `bindings/`), and it moves to Rust piece by piece
(decisions.md, 2026-11-24; `docs/research/runtime-implementation-language.md`).
This page says how CI checks the C for memory errors, undefined behaviour and
data races, and how the fuzz corpus pins the behaviour a Rust replacement must
keep. It came in with CHI-304.

## What runs

| Check | Where | What it catches |
| --- | --- | --- |
| `sanitizers (ubuntu-latest)` CI job | `.github/workflows/ci.yml` | The whole test suite, with every native program the tests build compiled with AddressSanitizer and UndefinedBehaviorSanitizer. A report stops the program, so the test fails; the job also fails on any report in its log files. |
| ThreadSanitizer fixtures | `compiler/chimi/tests/runtime_state.rs` | Shared runtime state touched outside its lock, from two to four real threads. |
| Sanitized C fixtures | `tests/native_list_fuzz.rs`, the native heap test in `tests/cli.rs`, `tests/runtime_fuzz.rs` | ASan and UBSan over the heap, the persistent list and the fuzz corpus. |
| `fuzz (<target>)` CI jobs | `scripts/fuzz-runtime.sh` | libFuzzer for two minutes on each of `json`, `text`, `decimal` and `duration`, from the committed corpus. |
| Corpus replay | `tests/runtime_fuzz.rs` | Every corpus input still gives its recorded output, on macOS and Linux. |

## Rules

- On Linux the sanitizers are required. A fixture whose sanitized build fails
  is a test failure there, never a quiet plain build. macOS keeps the plain
  fallback, because the sanitizers there depend on the installed toolchain.
- New C in the runtime or a binding package must pass the `sanitizers` job.
  Fix a report; do not suppress it. A report in vendored code (SQLite,
  mbedTLS) gets a ticket and a note here.
- A fuzz target's output line is a contract. When a subsystem moves to Rust,
  the Rust version replays the same corpus and must print the same lines,
  and the C version stays as the oracle until it does (CHI-305). An intended
  change of behaviour reblesses the corpus in the same PR, and the PR says why
  the lines changed.
- A fuzzer finding is fixed with its input added to the corpus, so the replay
  test keeps it fixed.

## Building programs with sanitizers

A debug build of the compiler reads `CHIMI_TEST_RUNTIME_SANITIZE`. Its one
value, `address,undefined`, compiles the runtime and every binding package the
program reaches with `-fsanitize=address,undefined -fno-sanitize-recover=all`
and links the sanitizer libraries. The archives get their own cache keys, so a
sanitized build never reuses a prebuilt or ordinary one. A release compiler
ignores the variable.

```sh
CHIMI_TEST_RUNTIME_SANITIZE=address,undefined cargo run -p chimi -- build app.chimi -o app
ASAN_OPTIONS=detect_leaks=0 ./app
```

- `detect_leaks=0`: the runtime keeps memory it owns until exit.
- ASan frames are several times larger, so under ASan each fiber gets a 4 MiB
  stack instead of 1 MiB (`CHIMI_FIBER_STACK_BYTES` in `runtime.c`), and the
  CI job raises the host thread's stack to 32 MiB. Programs then reach the
  same call depths as an ordinary build.
- ThreadSanitizer is not offered: it cannot follow the fibers' stack switches.
  It runs on the C fixtures only.
- The fiber switches are not announced to ASan
  (`__sanitizer_start_switch_fiber`), so a program that ends on a fiber
  prints "ASan is ignoring requested `__asan_handle_no_return`". That warning
  is expected. The CI job sends reports to log files (`log_path`) so it does
  not break tests that compare a program's stderr.

Under the variable, the tests skip the checks that cannot hold: that the
prebuilt runtime and binding packages are used, and that a program without
tasks stays under 1 MiB. Two Linux tests check the variable itself: a
sanitized build compiles its own runtime and binding packages and links the
sanitizer libraries, and an ordinary build after it does not reuse them.
`CHIMI_TEST_RUNTIME_DEFINES` changes only the runtime archive, never a binding
package.

## Fuzzing

`compiler/chimi/tests/fuzz/runtime-fuzz.c` includes `runtime.c` and calls the
intrinsics a compiled program calls:

| Target | Input | Output and invariants |
| --- | --- | --- |
| `json` | bytes, made Text with `Bytes.to_text_lossy` | `Json.decode`'s error offset and message, or the encoded value. Encoding is canonical: the encoded text decodes and encodes to itself. A number also shows its `Int` and `Decimal` conversions. |
| `text` | bytes | The valid UTF-8 prefix, the lossy Text, lower and upper case, both trims, and the grapheme lengths. Every result is valid UTF-8, lossy conversion keeps valid input, and the graphemes cover the text exactly. |
| `decimal` | two operands split at the first newline | Each operand read and printed, which reads back the same; then add, subtract, multiply and a divide whose scale and rounding come from the first byte. |
| `duration` | bytes | `Duration.parse`'s error, or the nanoseconds and printed form, which parses back to the same value. |

Each corpus file, `tests/fuzz/corpus/<target>.txt`, has one input per line:
the input quoted (printable ASCII as is, every other byte as `\xHH`), a tab,
and the recorded output.

```sh
# Fuzz one target for five minutes; findings land in the work directory.
scripts/fuzz-runtime.sh json 300 /tmp/fuzz-json
# Add a fixed finding to the corpus, then record its output.
/tmp/fuzz-json/runtime-fuzz json --files /tmp/fuzz-json/crash-... >> compiler/chimi/tests/fuzz/corpus/json.txt
CHIMI_FUZZ_BLESS=1 cargo test -p chimi --test runtime_fuzz
```

`fuzz-runtime.sh` needs clang with libFuzzer (Ubuntu: `libclang-rt-<version>-dev`).

The binding shims (TLS session input, certificate parsing, SCRAM) are not
fuzzed yet; that is the next part of CHI-304.
