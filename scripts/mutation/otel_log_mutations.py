"""Mutants of Otel.log, the live logs push (CHI-296, part 4).

Run: scripts/mutation/mutate.py scripts/mutation/otel_log_mutations.py
"""

CRATE = "compiler/chimi"

OTEL = "std/otel.chimi"
LOG = "std/log.chimi"


def mutant(name, edits):
    return dict(name=name, file=OTEL, edits=edits, targets=["std_otel"], filter="")


MUTATIONS = [
    mutant("the outer log is not written", [(
        "      outer.write(entry)\n",
        "",
    )]),
    mutant("the level is not filtered", [(
        "case Log.at_least(Log.level(entry), exporter.settings.log_level) {",
        "case True {",
    )]),
    mutant("a dropped log is not counted", [(
        "            _ -> Metrics.count(\"chimi.otel.dropped_logs\", 1, []) using { metrics: given_metrics }",
        "            _ -> Void",
    )]),
    mutant("waiting logs are not pushed when the body ends", [(
        "      flush_logs(exporter)\n",
        "",
    )]),
    mutant("a flush does not push", [(
        "        push_logs(settings, waiting, settings.flush_within)\n",
        "",
    )]),
    mutant("an interval does not push", [(
        "        push_logs(settings, waiting, settings.timeout)\n",
        "",
    )]),
    mutant("pushed logs stay waiting", [(
        "        push_logs(settings, waiting, settings.timeout)\n        ([], Clock.add(",
        "        push_logs(settings, waiting, settings.timeout)\n        (waiting, Clock.add(",
    )]),
    mutant("every number is a double", [(
        "        Ok(_) -> object([(\"intValue\", Json.text(digits))])",
        "        Ok(_) -> object([(\"doubleValue\", Value.Number(number))])",
    )]),
    mutant("warn has info's severity", [(
        "    Log.Level.Warn -> (13, \"WARN\")",
        "    Log.Level.Warn -> (9, \"WARN\")",
    )]),
    dict(name="at_least is strict", file=LOG, edits=[(
        "  rank(level) >= rank(minimum)",
        "  rank(level) > rank(minimum)",
    )], targets=["std_otel"], filter=""),
]
