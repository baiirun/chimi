"""Mutants of the scalar intrinsics in Rust (CHI-305 step 5, phase 5):
runtime/chimi-runtime/src/intrinsics/scalars.rs, which runtime.c's
invoke_intrinsic used to answer in its own switch. The language suite runs
each program on both backends, so a native mutant shows as a difference.

Run: scripts/mutation/mutate.py scripts/mutation/scalar_mutations.py
Results: scripts/mutation/scalar_results.txt.

Mutants of List.first and Order.min survived because std/list and std/order
define those functions in Chimi, so no program reached their intrinsics; the
arms (and Order.compare's and Order.max's) were removed instead.
"""

CRATE = "compiler/chimi"

SCALARS = "../../runtime/chimi-runtime/src/intrinsics/scalars.rs"


def language(name, edits):
    return dict(name=name, file=SCALARS, edits=edits, targets=["language"], filter="")


MUTATIONS = [
    language("remainder by -1 overflows", [
        ("        _ if right == -1 => Some(0),\n", "")]),
    language("modulo keeps the dividend's sign", [
        ("        if remainder != 0 && (remainder < 0) != (modulus < 0) {", "        if false {")]),
    language("Int.parse refuses the smallest Int", [
        ("    if negative { Ok(value) } else {", "    if negative { value.checked_neg().map(|value| -value).ok_or(StandardConstructor::INT_PARSE_OUT_OF_RANGE) } else {")]),
    language("Int.parse accepts a lone minus", [
        ("    if digits.is_empty() || !digits", "    if !digits")]),
    language("Float.parse accepts a bare point", [
        ("        cursor += 1;\n        if !digits(&mut cursor) {\n            return false;\n        }\n    }\n    if matches!",
         "        cursor += 1;\n        digits(&mut cursor);\n    }\n    if matches!")]),
    language("Float.to_int accepts 2^63", [
        ("..9_223_372_036_854_775_808.0).contains(&value)", "..=9_223_372_036_854_775_808.0).contains(&value)")]),
    language("Range.length of a reversed range is negative", [
        ("                    _ if range.end <= range.start => success(int(0)),\n", "")]),
    language("Range.contains includes its end", [
        ("range.start <= item && item < range.end", "range.start <= item && item <= range.end")]),
    language("round goes half up", [
        ("                    _ => round(value),", "                    _ => floor(value + 0.5),")]),
    language("Text.append drops the second", [
        ("                output.push_bytes(bytes_of(arguments[1]));\n", "")]),
]
