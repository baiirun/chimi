"""Mutants of the outcome map encoding (ts-0fa50c, CHI-177): a map whose keys
are not Text is written as `{"map":[[key,value],...]}`, each key once, on both
backends.

Run: scripts/mutation/mutate.py scripts/mutation/outcome_map_mutations.py
Each mutant must turn the named language test red.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
OUTCOME = "../../runtime/chimi-runtime/src/outcome.rs"
EXECUTION = "src/execution.rs"
SHAPES = "map_outcomes_render_their_entries_on_both_backends"


def language(name, file, edits, filter=SHAPES):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


MUTATIONS = [
    language("native writes an empty map as pairs", OUTCOME, [(
        "entries.first().is_none_or(|node| (*(**node).key).kind == ValueKind::TEXT)",
        "entries.first().is_some_and(|node| (*(**node).key).kind == ValueKind::TEXT)")]),
    language("native drops the comma between pairs", OUTCOME, [(
        'self.push(Step::Text(if index > 0 { b",[" } else { b"[" }));',
        'self.push(Step::Text(b"["));')]),
    language("native writes a pair value before its key", OUTCOME, [(
        "                    self.push(Step::Value((*node).value));\n"
        "                    self.push(Step::Text(b\",\"));\n"
        "                    self.push(Step::Value((*node).key));\n",
        "                    self.push(Step::Value((*node).key));\n"
        "                    self.push(Step::Text(b\",\"));\n"
        "                    self.push(Step::Value((*node).value));\n")]),
    language("interpreter writes pairs in reverse key order", EXECUTION, [(
        "                for (index, (key, value)) in entries.iter().enumerate().rev() {",
        "                for (index, (key, value)) in entries.iter().rev().enumerate().rev() {")]),
    language("interpreter writes a pair value before its key", EXECUTION, [(
        "                    steps.push(JsonStep::Value(value));\n"
        "                    steps.push(JsonStep::Punctuation(\",\"));\n"
        "                    steps.push(JsonStep::Value(key));\n",
        "                    steps.push(JsonStep::Value(key));\n"
        "                    steps.push(JsonStep::Punctuation(\",\"));\n"
        "                    steps.push(JsonStep::Value(value));\n")]),
    language("interpreter writes Text-keyed maps as pairs", EXECUTION, [(
        "                Some(fields) => ExecutionValue::Record(fields),",
        "                Some(_) => ExecutionValue::Map(\n"
        "                    entries\n"
        "                        .iter()\n"
        "                        .map(|(key, value)| (public_value(key), public_value(value)))\n"
        "                        .collect(),\n"
        "                ),")]),
]
