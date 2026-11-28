"""Mutants of the outcome record and the operation profile in Rust (CHI-305
step 5, phase 8, ninth PR: runtime/chimi-runtime/src/outcome.rs and
reports.rs). The language tests compare each program's outcome record on
both backends, so a value the native runtime writes differently shows up as
a difference from the interpreter.

Run: scripts/mutation/mutate.py scripts/mutation/outcome_mutations.py
Results: scripts/mutation/outcome_results.txt.
"""

CRATE = "compiler/chimi"

OUTCOME = "../../runtime/chimi-runtime/src/outcome.rs"
REPORTS = "../../runtime/chimi-runtime/src/reports.rs"


def mutant(name, file, old, new, targets=("language", "cli")):
    return dict(name=name, file=file, edits=[(old, new)], targets=list(targets), filter="")


MUTATIONS = [
    mutant("a list writes its items in reverse", OUTCOME,
           "                    for index in (0..payload.list.length).rev() {",
           "                    for index in 0..payload.list.length {"),
    mutant("a record drops its field names", OUTCOME,
           "                    steps.fields(payload.record.fields, payload.record.length, true, b\"}\");",
           "                    steps.fields(payload.record.fields, payload.record.length, false, b\"}\");"),
    mutant("a map of other keys is written as an object", OUTCOME,
           "            let text_keyed = entries.first().is_none_or(|node| (*(**node).key).kind == ValueKind::TEXT);",
           "            let text_keyed = true;"),
    mutant("a constructed value drops its variant", OUTCOME,
           "                    if !constructed.variant_name.is_null() {",
           "                    if false {"),
    mutant("a decimal keeps its trailing zeros", OUTCOME,
           "                    out.json(decimal.normalized_text().as_bytes());",
           "                    out.json(decimal.text().as_bytes());"),
    mutant("bytes swap their hex digits", OUTCOME,
           "out.bytes(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]]);",
           "out.bytes(&[HEX[usize::from(byte & 0xf)], HEX[usize::from(byte >> 4)]]);"),
    mutant("a set writes no separators", OUTCOME,
           "                        steps.push(Step::Value((*node).key));\n                        if index > 0 {",
           "                        steps.push(Step::Value((*node).key));\n                        if false {"),
    mutant("a failure in a test build has no rendering", OUTCOME,
           "                if CHIMI_TEST_COUNT > 0 {",
           "                if CHIMI_TEST_COUNT > u32::MAX - 1 {", targets=("testing",)),
    mutant("a defect names no origin", OUTCOME,
           "                    out.unsigned(u64::from(node.origin));",
           "                    out.unsigned(0);"),
    mutant("an outcome drops its cause", OUTCOME,
           "            if !cause.is_null() && cause_has_detail(cause) {",
           "            if false && !cause.is_null() && cause_has_detail(cause) {", targets=("std_modules",)),
    mutant("the profile keeps nodes never evaluated", REPORTS,
           ".filter(|&node| (*rows.add(node as usize)).evaluations != 0)",
           ".filter(|_| true)", targets=("cli",)),
    mutant("the profile counts no allocations", REPORTS,
           "                (b\",\\\"allocations\\\":\", row.allocations),",
           "                (b\",\\\"allocations\\\":\", 0),"),
]
