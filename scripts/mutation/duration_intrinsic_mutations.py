"""Mutants of the std/duration intrinsics in Rust (CHI-305 step 5, phase 5,
fourth PR; runtime/chimi-runtime/src/intrinsics/durations.rs). The language
suite's duration tests run a generated set through both backends and compare
defect messages too; the parse checks are in the runtime fuzz corpus. Mutants
of the C these replaced were retargeted in duration and
rust_decimal_duration.

Run: scripts/mutation/mutate.py scripts/mutation/duration_intrinsic_mutations.py
Results: scripts/mutation/duration_intrinsic_results.txt.
"""

CRATE = "compiler/chimi"
DURATIONS = "../../runtime/chimi-runtime/src/intrinsics/durations.rs"


def language(name, old, new):
    return dict(name=name, file=DURATIONS, edits=[(old, new)], targets=["language"], filter="duration")


def fuzz(name, old, new):
    return dict(name=name, file=DURATIONS, edits=[(old, new)], targets=["runtime_fuzz"], filter="")


MUTATIONS = [
    language("constructors take their units in reverse", "Unit::DESCENDING[5 - offset as usize]",
             "Unit::DESCENDING[offset as usize]"),
    language("to_ conversions return nanoseconds", "duration::whole_units(value, unit)", "value"),
    language("a subtract overflow is written as an add", "if add { b\" + \" } else { b\" - \" }", "b\" + \""),
    language("a multiply overflow names the wrong factor", ".text(b\" * \").integer(other)",
             ".text(b\" * \").integer(other.wrapping_add(1))"),
    language("a divide by zero names another divisor", ".text(left).text(b\" / 0\")", ".text(left).text(b\" / 1\")"),
    language("the minimum divided by -1 wraps", "match value.checked_div(other) {",
             "match Some(value.wrapping_div(other)) {"),
    language("absolute keeps a negative duration", "if negate || value < 0 { -value } else { value }",
             "if negate { -value } else { value }"),
    language("to_text drops the last character", "success(text_value(left.as_ptr().cast(), left.len()))",
             "success(text_value(left.as_ptr().cast(), left.len() - 1))"),
    fuzz("text that is not UTF-8 parses as Empty", ".map_err(|_| ParseFailure::Invalid)",
         ".map_err(|_| ParseFailure::Empty)"),
]
