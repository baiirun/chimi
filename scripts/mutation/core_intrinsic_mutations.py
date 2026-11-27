"""Mutants of the native cores of Debug.crash, Option.unwrap_or_fail,
std/text and std/list in Rust (CHI-305 step 5, phase 5, fifth PR;
runtime/chimi-runtime/src/intrinsics/core.rs). The std module tests run
std/text and std/list on both backends and compare them; the language suite
compares defect messages. Mutants of the C these replaced were retargeted in
cause, lane_fixes, rust_text and std_modules. Debug.crash's copy stopping at
U+0000 has no mutant: a control's message is a C string, read up to its first
NUL, so the bytes after it are never seen either way.

Run: scripts/mutation/mutate.py scripts/mutation/core_intrinsic_mutations.py
Results: scripts/mutation/core_intrinsic_results.txt.
"""

CRATE = "compiler/chimi"
CORE = "../../runtime/chimi-runtime/src/intrinsics/core.rs"


def std(name, old, new):
    return dict(name=name, file=CORE, edits=[(old, new)], targets=["std_modules"], filter="standard_")


def language(name, old, new, filter=""):
    return dict(name=name, file=CORE, edits=[(old, new)], targets=["language"], filter=filter)


MUTATIONS = [
    std("concat skips each part's first byte", "output.push_bytes(bytes_of(part));",
        "output.push_bytes(&bytes_of(part)[bytes_of(part).len().min(1)..]);"),
    std("byte_slice keeps the text from the start", "success(new_text(&bytes[from..to]))",
        "success(new_text(&bytes[..to]))"),
    std("byte_slice checks only the start bound", "for bound in [from, to] {", "for bound in [from] {"),
    std("contains always finds", "find(bytes_of(arguments[0]), needle).is_some()",
        "find(bytes_of(arguments[0]), needle).is_some() || true"),
    std("starts_with checks the end", "bytes_of(arguments[0]).starts_with(bytes_of(arguments[1]))",
        "bytes_of(arguments[0]).ends_with(bytes_of(arguments[1]))"),
    std("ends_with checks the start", "bytes_of(arguments[0]).ends_with(bytes_of(arguments[1]))",
        "bytes_of(arguments[0]).starts_with(bytes_of(arguments[1]))"),
    std("split keeps the separator", "rest = &rest[found + separator.len()..];", "rest = &rest[found + 1.min(separator.len())..];"),
    std("replace drops the replacement", "output.push_bytes(replacement);", ""),
    std("List.get reads one past the index", "Ok(index) if index < length => list::get(arguments[0], index),",
        "Ok(index) if index < length => list::get(arguments[0], index + 1),"),
    std("List.slice ignores the start", "success(list::slice(current_carrier(), arguments[0], from as u32, to as u32))",
        "success(list::slice(current_carrier(), arguments[0], 0, to as u32))"),
    std("List.set writes the next slot",
        "let vector = list::set(window.vector, window.start + index, arguments[2]);",
        "let vector = list::set(window.vector, window.start + index + (index + 1 < window.length) as u32, arguments[2]);"),
    language("List.set names the index as the bound", ".integer(i64::from(window.length))", ".integer(index)"),
    language("byte_slice names the wrong bound", ".integer(bound as i64)", ".integer(bound as i64 + 1)"),
]
