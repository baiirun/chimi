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
| `sanitizers (<shard>)` CI jobs | `.github/workflows/ci.yml`, `scripts/sanitizer-shard.sh` | The whole test suite, in six shards since it takes about an hour and a half under the sanitizers, with every native program the tests build compiled with AddressSanitizer and UndefinedBehaviorSanitizer. A report stops the program, so the test fails; the job also fails on any report in its log files. |
| ThreadSanitizer fixtures | `compiler/chimi/tests/runtime_state.rs` | Shared runtime state touched outside its lock, from two to four real threads. |
| Sanitized C fixtures | `tests/native_list_fuzz.rs`, the native heap test in `tests/cli.rs`, `tests/runtime_fuzz.rs` | ASan and UBSan over the heap, the persistent list and the fuzz corpora. |
| `fuzz (<target>)` CI jobs | `scripts/fuzz-runtime.sh` | libFuzzer for two minutes on each runtime and binding target, from the committed corpus. |
| Corpus replay | `tests/runtime_fuzz.rs` | Every corpus input still gives its recorded output, on macOS and Linux. The binding harness's objects are cached under `target/`, since SQLite alone takes over a minute to compile under ASan. |

## Rules

- On Linux the sanitizers are required. A fixture whose sanitized build fails
  is a test failure there, never a quiet plain build. macOS keeps the plain
  fallback, because the sanitizers there depend on the installed toolchain.
- New C in the runtime or a binding package must pass the `sanitizers` jobs.
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

### The binding shims

`compiler/chimi/tests/fuzz/binding-fuzz.c` is a small binding host: it
implements the binding API (`chimi_binding.h`) over plain memory and calls
the shims' own entry points, linked with the `tls` and `sqlite` packages'
code. It shares the replay driver and corpus format with `runtime-fuzz.c`
(`fuzz-main.h`, `fuzz-output.h`).

| Target | Input | Output and invariants |
| --- | --- | --- |
| `tls-client` | a server's reply to the client's ClientHello; the first byte also splits it into two feeds | Where the handshake stopped: `needs input`, `complete`, or the failure code and message. A failure always records its code. |
| `tls-server` | a client's first flight, split the same way | The same, for a server with the fixture certificate. |
| `tls-records` | a script, one operation a byte, over a real handshake: send the next bytes, move ciphertext in pieces of a size, read into a buffer of a size, close | How many bytes the client read and whether it saw the close. It must read exactly the bytes sent, in order, and see the close exactly when one was sent. |
| `tls-roots` | a PEM file | `tls_check_roots`'s count, or its message. |
| `scram` | lengths, then a password, a salt, an iteration count and an auth message | The client proof and server signature in hex, or the failure. The corpus holds the RFC 7677 vector. |
| `sqlite` | a statement (its valid UTF-8 prefix) | `prepare`'s failure, `none`, or the statement's parameter count. The shim refuses a second statement exactly when SQLite finds a statement, an error or a NUL after the first. |

The TLS targets read `ca.pem`, `server.pem` and `server.key` from
`tests/fixtures/tls` (`CHIMI_FUZZ_TLS_FIXTURES`). Their outputs do not
depend on the handshake's random values, so the replay is deterministic.

The other shims parse nothing a peer controls: `net` and `exec` hand their
arguments to the operating system, `terminal` reads no input, and the
postgres gate only stores bytes. SQLite and mbedTLS themselves are fuzzed
upstream (OSS-Fuzz); these targets cover the shims and how they call them.

The `sqlite` target's first runs found that std/sqlite's own reading of the
text after a statement disagreed with SQLite's: a `/*` at the very end, and
a vertical tab after a space. The shim now asks SQLite whether the rest
holds anything (`only_blank` in `sqlite.c`), and both inputs are in the
corpus.
