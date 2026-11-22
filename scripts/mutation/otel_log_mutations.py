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
        "Task.start(fn() { flush_logs(exporter) })",
        "Task.start(fn() { Void })",
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
    mutant("waiting entries are not capped", [(
        "case List.length(waiting) < settings.log_buffer {",
        "case True {",
    )]),
    mutant("the last pushes run one after the other", [(
        "      let logs = Task.start(fn() { flush_logs(exporter) })\n      push_within(settings, settings.flush_within)\n",
        "      flush_logs(exporter)\n      let logs = Task.start(fn() { Void })\n      push_within(settings, settings.flush_within)\n",
    )]),
    mutant("a dropped entry in the process is not counted", [(
        "          Metrics.count(\"chimi.otel.dropped_logs\", 1, [])\n          (waiting, deadline)",
        "          (waiting, deadline)",
    )]),
    mutant("a whole number is written as spelled", [(
        "Ok(whole) -> object([(\"intValue\", Json.text(Int.to_text(whole)))])",
        "Ok(whole) -> object([(\"intValue\", Json.text(spelling(number)))])",
    )]),
    mutant("every number is a double", [(
        "Ok(whole) -> object([(\"intValue\", Json.text(Int.to_text(whole)))])",
        "Ok(whole) -> object([(\"doubleValue\", Value.Number(number))])",
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
