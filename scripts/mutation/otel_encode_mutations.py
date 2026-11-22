"""Mutants of std/otel's metrics encoding (CHI-296, PR 1).

Run: scripts/mutation/mutate.py scripts/mutation/otel_encode_mutations.py
"""

CRATE = "compiler/chimi"

OTEL = "std/otel.chimi"


def mutant(name, edits):
    return dict(name=name, file=OTEL, edits=edits, targets=["std_otel"], filter="")


MUTATIONS = [
    mutant("a 64-bit integer is written as a number", [(
        "    Value.Number(number) -> Json.text(spelling(number))",
        "    Value.Number(number) -> Value.Number(number)",
    )]),
    mutant("one name's label sets are separate metrics", [(
        "          True -> List.set(done, last, List.push(group, next))",
        "          True -> List.push(done, [next])",
    )]),
    mutant("a name's kind change does not start a metric", [(
        "first.name == next.name and first.kind == next.kind",
        "first.name == next.name",
    )]),
    mutant("a zero dropped total is written", [(
        "        Success(0) -> None",
        "        Success(-1) -> None",
    )]),
    mutant("a histogram of the wrong shape is read", [(
        "case List.length(buckets) == List.length(bounds) + 1 {",
        "case List.length(buckets) > 0 {",
    )]),
    mutant("a time before 1970 is accepted", [(
        "      Success(nanoseconds) -> case nanoseconds < 0 {",
        "      Success(nanoseconds) -> case nanoseconds < -1 {",
    )]),
    mutant("a count is not monotonic", [(
        "      (\"isMonotonic\", Json.boolean(True)),\n",
        "",
    )]),
    mutant("the service name is ignored", [(
        "attribute(\"service.name\", service_name)",
        "attribute(\"service.name\", \"unknown_service\")",
    )]),
    mutant("the schema is not checked", [(
        "    _ -> Failure(DocumentError.Invalid(\"not a chimi.metrics.v1 document\"))",
        "    _ -> Void",
    )]),
    mutant("the end time is the start time", [(
        "    (\"timeUnixNano\", Json.text(times.end)),",
        "    (\"timeUnixNano\", Json.text(times.start)),",
    )]),
]
