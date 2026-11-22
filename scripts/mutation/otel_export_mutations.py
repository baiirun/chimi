"""Mutants of Otel.export, the live metrics push (CHI-296, part 3).

Run: scripts/mutation/mutate.py scripts/mutation/otel_export_mutations.py
"""

CRATE = "compiler/chimi"

OTEL = "std/otel.chimi"


def mutant(name, edits):
    return dict(name=name, file=OTEL, edits=edits, targets=["std_otel"], filter="")


MUTATIONS = [
    mutant("no push when the body ends", [(
        "    Task.cancel(pushing)\n    push_within(settings, settings.flush_within)\n",
        "    Task.cancel(pushing)\n",
    )]),
    mutant("no periodic push", [(
        "    Clock.sleep(settings.interval)\n    push_within(settings, settings.timeout)\n",
        "    Clock.sleep(settings.interval)\n",
    )]),
    mutant("recording is not turned on first", [(
        "  let _ = Metrics.runner_document()\n  let exporter",
        "  let exporter",
    )]),
    mutant("a failed push is not counted", [(
        "    _ -> Metrics.count(\"chimi.otel.failed_exports\", 1, [])",
        "    _ -> Void",
    )]),
    mutant("any status counts as pushed", [(
        "status >= 200 and status < 300",
        "status >= 200",
    )]),
    mutant("the settings' headers are not sent", [(
        "List.append([(\"content-type\", \"application/json\")], settings.headers)",
        "[(\"content-type\", \"application/json\")]",
    )]),
    mutant("metrics go to the endpoint's root", [(
        "\"${settings.endpoint}/v1/metrics\"",
        "\"${settings.endpoint}/\"",
    )]),
    mutant("the last push is not bounded by flush_within", [(
        "    push_within(settings, settings.flush_within)\n",
        "    push_within(settings, Duration.seconds(30))\n",
    )]),
    mutant("the service name is not sent", [(
        "metrics_request(Metrics.runner_document(), settings.service_name)",
        "metrics_request(Metrics.runner_document(), \"unknown_service\")",
    )]),
]
