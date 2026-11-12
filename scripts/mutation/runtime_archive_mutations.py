"""Mutants of the per-compiler native runtime archive (ts-1db9f0):
compiler/chimi/build.rs, src/backend/runtime_archive_key.rs and the
prebuilt-or-project-cache choice in src/backend/native.rs.

Run: scripts/mutation/mutate.py scripts/mutation/runtime_archive_mutations.py
Each mutant runs the two prebuilt-runtime CLI tests and the build-cache test.
"""

CRATE = "compiler/chimi"

FILTER = "runtime"
TESTS = ["cli"]

MUTATIONS = [
    dict(
        name="prebuilt archive used whatever its key",
        file="src/backend/native.rs",
        edits=[("(!PREBUILT_RUNTIME_KEY.is_empty() && PREBUILT_RUNTIME_KEY == key)",
                "(!PREBUILT_RUNTIME_KEY.is_empty() || PREBUILT_RUNTIME_KEY == key)")],
        targets=TESTS, filter=FILTER,
    ),
    dict(
        name="prebuilt archive never used",
        file="src/backend/native.rs",
        edits=[("(!PREBUILT_RUNTIME_KEY.is_empty() && PREBUILT_RUNTIME_KEY == key)",
                "(PREBUILT_RUNTIME_KEY.is_empty() && PREBUILT_RUNTIME_KEY == key)")],
        targets=TESTS, filter=FILTER,
    ),
    dict(
        name="key ignores the C compiler identity",
        file="src/backend/runtime_archive_key.rs",
        edits=[("        inputs.compiler_identity.as_bytes(),\n", "        b\"\",\n")],
        targets=TESTS, filter=FILTER,
    ),
    dict(
        name="build script skips prebuilding",
        file="build.rs",
        edits=[("        Ok(Some(built)) => built,\n", "        Ok(Some(_)) => (Vec::new(), String::new()),\n")],
        targets=TESTS, filter=FILTER,
    ),
    dict(
        name="build script keys a different source than the compiler embeds",
        file="build.rs",
        edits=[("        runtime_source: &runtime_source,\n", "        runtime_source: &runtime_header,\n")],
        targets=TESTS, filter=FILTER,
    ),
    dict(
        name="fallback still compiles but is not cached",
        file="src/backend/native.rs",
        edits=[('            &runtime_key,\n            "a",\n', '            &format!("{runtime_key}-{}", std::process::id()),\n            "a",\n')],
        targets=TESTS, filter=FILTER,
    ),
]
