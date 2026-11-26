"""Mutants of the std/json port (992332f): the Rust model in src/json.rs and its
wiring in src/execution/, the C twin in runtime/native/runtime.c, and std/json.chimi.
Rust and C sides are mutated separately because std_modules compares both
backends against the same expected files.

Run: scripts/mutation/mutate.py --revision 992332f scripts/mutation/std2_json_mutations.py
Each mutant must turn a json unit test, a std-tests/json program (both
backends), or a language.rs json test red.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
COMPARE = "../../runtime/chimi-runtime/src/heap/compare.rs"
MODEL = "src/json.rs"
STRUCTURAL = "src/execution/structural.rs"


def std(name, file, edits):
    # Rust model: the json.rs unit tests.
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter="json")


def programs(name, file, edits):
    # C runtime and wiring: only the std-tests programs compare backends.
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_")


def lang(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter="json")


MUTATIONS = [
    std("Rust number_to_int reads the spelling (1.0 is NotInteger)", MODEL, [(
        "    if number.exponent < 0 {\n        return Err(NumberConversionError::NotInteger);",
        "    if text.contains('.') || number.exponent < 0 {\n        return Err(NumberConversionError::NotInteger);")]),
    programs("C number_to_int reads the spelling (1.0 is NotInteger)", RUNTIME, [(
        "if (number.exponent < 0) { *not_integer = true; return false; }",
        "if (number.exponent < 0 || strchr(text, '.')) { *not_integer = true; return false; }")]),
    programs("C number_to_int rejects Int min (no extra magnitude for negatives)", RUNTIME, [(
        "(unsigned __int128)INT64_MAX + (number.negative ? 1u : 0u)",
        "(unsigned __int128)INT64_MAX")]),
    std("Rust number_to_int calls a 19-digit integer OutOfRange", MODEL, [(
        "if length + number.exponent > 19 {",
        "if length + number.exponent > 18 {")]),
    std("Rust number_to_decimal drops the written scale", MODEL, [(
        "    let scale = number\n        .written_scale\n        .max(0)",
        "    let scale = (-number.exponent)\n        .max(0)")]),
    programs("C number_to_decimal accepts 39 fraction digits", RUNTIME, [(
        "|| -number.exponent > max_scale) return false;",
        "|| -number.exponent > max_scale + 1) return false;")]),
    programs("C exponent sign ignored", RUNTIME, [(
        "written_negative = *cursor == '-'; if",
        "written_negative = false; if")]),
    programs("Rust encode escapes DEL", MODEL, [(
        "character if character < '\\u{20}' => {",
        "character if character < '\\u{20}' || character == '\\u{7f}' => {")]),
    programs("C encode escapes DEL", RUNTIME, [(
        "        if (*cursor < 0x20) {\n          char escaped[7]; snprintf(escaped, sizeof(escaped), \"\\\\u%04x\", *cursor);",
        "        if (*cursor < 0x20 || *cursor == 0x7f) {\n          char escaped[7]; snprintf(escaped, sizeof(escaped), \"\\\\u%04x\", *cursor);")]),
    std("Rust decode accepts a duplicate key", MODEL, [(
        "if !names.insert(name.clone()) {",
        "if !names.insert(name.clone()) && false {")]),
    programs("C decode accepts a duplicate key", RUNTIME, [(
        "if (map_get_node(map, key)) {\n      TextBuffer message",
        "if (0 && map_get_node(map, key)) {\n      TextBuffer message")]),
    std("Rust nesting limit is 129", MODEL, [(
        "const MAX_JSON_DEPTH: usize = 128;",
        "const MAX_JSON_DEPTH: usize = 129;")]),
    programs("C nesting limit is 129", RUNTIME, [(
        "if (depth > 128) return json_fail(",
        "if (depth > 129) return json_fail(")]),
    std("Rust encode keeps object members in stored order", MODEL, [(
        "            fields.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));\n",
        "")]),
    programs("Rust Json.Number equality compares spelling", STRUCTURAL, [(
        "crate::json::compare_numbers(left, right)",
        "left.as_bytes().cmp(right.as_bytes())")]),
    programs("C Json.Number equality compares spelling", COMPARE, [(
        "order = super::json_number::compare(c_bytes(l.json_number), c_bytes(r.json_number));",
        "order = order_of(c_bytes(l.json_number), c_bytes(r.json_number));")]),
]
