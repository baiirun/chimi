"""Mutants for the std/otel collector-down test after it moved the refused
and 503 cases onto the virtual clock (CHI-318). Each must still be caught."""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]
F = "std/otel.chimi"


def mutant(name, edits):
    return dict(name=name, file=F, edits=edits, targets=["std_otel"],
                filter="a_collector_that_is_down")


MUTATIONS = [
    mutant("a 5xx reply counts as a success", [
        ("      status >= 200 and status < 300\n    }, limit)",
         "      status >= 200\n    }, limit)"),
    ]),
    mutant("a failed push is counted twice", [
        ("    _ -> Metrics.count(\"chimi.otel.failed_exports\", 1, [])\n  }\n}",
         "    _ -> Metrics.count(\"chimi.otel.failed_exports\", 2, [])\n  }\n}"),
    ]),
    mutant("the periodic push is skipped", [
        ("    Clock.sleep(settings.interval)\n    push_within(settings, settings.timeout)",
         "    Clock.sleep(settings.interval)\n    Void"),
    ]),
]
