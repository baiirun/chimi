"""Mutants of std/otel's metrics encoding (CHI-296, PR 1).

Run: scripts/mutation/mutate.py scripts/mutation/otel_encode_mutations.py
"""

CRATE = "compiler/chimi"

OTEL = "std/otel.chimi"


def mutant(name, edits):
    return dict(name=name, file=OTEL, edits=edits, targets=["std_otel"], filter="")


EXPORT = "src/metrics_export.rs"


MUTATIONS = [
    dict(name="the CLI writes labels in document order", file=EXPORT, edits=[(
        "        labels.sort();\n",
        "",
    )], targets=["std_otel"], filter=""),
    mutant("a number above 2^64 - 1 counts as unsigned", [(
        "(length < 20 or (length == 20 and digits <= \"18446744073709551615\"))",
        "length <= 20",
    )]),
    mutant("a fraction or exponent counts as unsigned", [(
        "[\"-\", \".\", \"e\", \"E\"]",
        "[\"-\"]",
    )]),
    mutant("a 20-digit number below 2^64 is refused", [(
        "(length < 20 or (length == 20 and digits <= \"18446744073709551615\"))",
        "length < 20",
    )]),
    mutant("a non-object entry gets the series message", [(
        "      _ -> Failure(DocumentError.Invalid(\"metric ${index} is not an object\"))",
        "      _ -> Failure(DocumentError.Invalid(\"metric ${index} is not a count, gauge or observation\"))",
    )]),
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
        "is_unsigned(digits) and digits != \"0\"",
        "is_unsigned(digits)",
    )]),
    mutant("a histogram of the wrong shape is read", [(
        "case List.length(buckets) == List.length(bounds) + 1 {",
        "case List.length(buckets) > 0 {",
    )]),
    mutant("a time before 1970 is accepted", [(
        "case (Text.starts_with(digits, \"-\"), is_unsigned(digits)) {",
        "case (False, is_unsigned(digits)) {",
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
