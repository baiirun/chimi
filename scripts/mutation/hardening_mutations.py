"""Mutants of the runtime hardening (CHI-304): sanitized test builds and the
runtime fuzz corpus.

Run: scripts/mutation/mutate.py scripts/mutation/hardening_mutations.py
The build mutants run the sanitized-build tests in `cli` and `bindings`; the
runtime mutants run the corpus replay in `runtime_fuzz`, which must report a
changed output or a broken invariant.
"""

CRATE = "compiler/chimi"

N = "src/backend/native.rs"
K = "src/backend/runtime_archive_key.rs"
C = "runtime/native/runtime.c"
BYTES_INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/bytes.rs"
JSON_INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/json.rs"


def build(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["cli", "bindings"], filter="sanitized")


def corpus(name, edits, file=C):
    return dict(name=name, file=file, edits=edits, targets=["runtime_fuzz"], filter="")


MUTATIONS = [
    build("a sanitized archive keeps the ordinary key", N, [(
        "        if self.compile_flags.is_empty() {\n            return key;",
        "        if true {\n            return key;",
    )]),
    build("the link leaves out the sanitizer libraries", N, [(
        "        build.link_flags.push(sanitize);",
        "        let _ = sanitize;",
    )]),
    build("binding packages compile without the test flags", K, [(
        "                        .args(extra_flags)\n",
        "",
    )]),
    corpus('JSON encoding stops escaping quotes', [('b\'"\' => output.push_bytes(b"\\\\\\""),',
        'b\'"\' => output.push_bytes(b"\\""),')], JSON_INTRINSICS),
    corpus("JSON nesting limit off by one", [(
        "if (depth > 128) return json_fail",
        "if (depth > 127) return json_fail",
    )]),
    corpus("a decimal prints without its sign", [(
        "  if (value.negative) output[length++] = '-';\n  for (size_t index = count;",
        "  for (size_t index = count;",
    )]),
    corpus("graphemes never join a combining mark", [(
        "    if (grapheme_breaks_before(&cluster, next)) return offset;",
        "    if (grapheme_breaks_before(&cluster, next) || next.grapheme_break != 0) return offset;",
    )]),
    corpus('lossy conversion keeps invalid bytes', [('output.push_bytes(if complete {', 'output.push_bytes(if true {')], BYTES_INTRINSICS),
    corpus("durations under a second print in seconds", [(
        "  if (magnitude < (uint64_t)DURATION_UNITS[2]) {",
        "  if (magnitude < (uint64_t)DURATION_UNITS[3]) {",
    )]),
]
