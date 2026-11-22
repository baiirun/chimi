"""Mutants of `chimi trace export --otlp` (Phase 7c step 7, PR 2).

Run: scripts/mutation/mutate.py scripts/mutation/trace_export_mutations.py
"""

CRATE = "compiler/chimi"

EXPORT = "src/trace_export.rs"


def mutant(name, edits, filter=""):
    return dict(name=name, file=EXPORT, edits=edits, targets=["trace_export"], filter=filter)


MUTATIONS = [
    mutant("times ignore the run's Unix start", [(
        "    u64::try_from(start.saturating_add(time)).unwrap_or(0)",
        "    u64::try_from(time).unwrap_or(start as u64 * 0)",
    )]),
    mutant("a completion does not end its span", [(
        "            span.end = Some(time);\n            span.completed = true;",
        "            span.completed = true;",
    )]),
    mutant("an unfinished operation does not take its children's end", [(
        "                parent.end = Some(parent.end.unwrap_or(parent.start).max(end));",
        "                parent.end = Some(parent.end.unwrap_or(parent.start));",
    )]),
    mutant("a second run under one execution id is merged", [(
        "            Some(_) if event == \"computation_started\" => {",
        "            Some(_) if false => {",
    )]),
    mutant("a parent is not written", [(
        "            let _ = write!(output, \",\\\"parentSpanId\\\":\\\"{parent:016x}\\\"\");",
        "            let _ = parent;",
    )]),
    mutant("a defect is not an error", [(
        "Some(outcome @ (\"expected_failure\" | \"defect\"))",
        "Some(outcome @ \"expected_failure\")",
    )]),
    mutant("every completed span is an error", [(
        "        if !span.completed && span.dependency.is_none() {",
        "        if span.dependency.is_none() {",
    )]),
    mutant("the service name is ignored", [(
        "service_name.unwrap_or(&self.computation)",
        "{ let _ = service_name; &self.computation }",
    )]),
    mutant("an untimed event is placed at the origin", [(
        "        let time = record.attribute_number(\"time_ns\").ok_or_else(|| {",
        "        let time = record.attribute_number(\"time_ns\").or(Some(0)).ok_or_else(|| {",
    )]),
    mutant("the source is not named", [(
        "        if let Some(place) = locate(span.file_id as u32, span.source.0 as u32) {",
        "        if let Some(place) = None::<String> {",
    )]),
    mutant("each span gets its own trace id", [(
        "            self.write_span(output, span, &trace_id, locate);",
        "            self.write_span(output, span, &format!(\"{trace_id:.31}{index}\"), locate);",
    )]),
]
