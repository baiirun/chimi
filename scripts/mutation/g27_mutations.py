"""Mutants of G27 (CHI-220): a process's own address, mailbox metrics, and
monitors (docs/specification/process-monitors-and-metrics.md).

Run: scripts/mutation/mutate.py scripts/mutation/g27_mutations.py
Results: scripts/mutation/g27_results.txt.
"""

CRATE = "compiler/chimi"

MONITOR = "../../runtime/chimi-fiber/src/scheduler/process/monitor.rs"
PROCESS = "../../runtime/chimi-fiber/src/scheduler/process.rs"
RUNTIME_C = "runtime/native/runtime.c"


def lab(scenario):
    return dict(
        command=["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
                 "--scenario", scenario],
        expect=[scenario],
    )


MUTATIONS = [
    dict(name="a notice counts against the capacity", file=PROCESS,
         edits=[("let messages = incarnation.mailbox.len() - incarnation.notices;",
                 "let messages = incarnation.mailbox.len();")],
         **lab("MB15")),
    dict(name="a notice is refused at a full mailbox", file=MONITOR,
         edits=[(".filter(|record| record.accepting)",
                 ".filter(|record| record.accepting && record.mailbox.len() - record.notices < record.spec.mailbox_capacity.get() as usize)")],
         **lab("MB15")),
    dict(name="a notice jumps the queue", file=MONITOR,
         edits=[("record.mailbox.push_back(Entry::Notice(notice));",
                 "record.mailbox.push_front(Entry::Notice(notice));")],
         **lab("MB15")),
    dict(name="a failure is reported as a return", file=MONITOR,
         edits=[("TerminalCause::Failed => Self::Failed,", "TerminalCause::Failed => Self::Returned,")],
         **lab("MB14")),
    dict(name="a stop is reported as a defect", file=MONITOR,
         edits=[("TerminalCause::Cancelled | TerminalCause::Forced => Self::Stopped,",
                 "TerminalCause::Cancelled | TerminalCause::Forced => Self::Defected,")],
         **lab("MB14")),
    dict(name="nothing to watch reads as a return", file=MONITOR,
         edits=[("                    down: Down::Unavailable,\n", "                    down: Down::Returned,\n")],
         **lab("MB14")),
    dict(name="unmonitor leaves a queued notice", file=MONITOR,
         edits=[("        if let Some(Entry::Notice(notice)) = queued.and_then(|index| record.mailbox.remove(index)) {",
                 "        if let Some(Entry::Notice(notice)) = queued.and(None) {")],
         **lab("MB16")),
    dict(name="a watcher's own monitors outlive it", file=MONITOR,
         edits=[("                self.unlink(&record, monitor);\n                self.release_message(record.handle);\n            }\n        }",
                 "                self.unlink(&record, monitor);\n            }\n        }")],
         **lab("MB17")),
    dict(name="queued counts the message just taken", file=MONITOR,
         edits=[("(record.mailbox.len(), record.spec.mailbox_capacity.get())",
                 "(record.mailbox.len() + 1, record.spec.mailbox_capacity.get())")],
         **lab("MB13")),
    dict(name="capacity reports the backlog", file=RUNTIME_C,
         edits=[(": (int64_t)request.capacity));", ": (int64_t)request.queued));")],
         **lab("MB13")),
    dict(name="the own address routes by name", file=RUNTIME_C,
         edits=[("      if (request.status != CHIMI_RESPONSE_OK) return defect(\"Process.address scheduler request failed\", call_node);\n      return success(process_address_value(CHIMI_ADDRESS_DIRECT, request.address));",
                 "      if (request.status != CHIMI_RESPONSE_OK) return defect(\"Process.address scheduler request failed\", call_node);\n      return success(process_address_value(CHIMI_ADDRESS_NAMED, request.address));")],
         **lab("MB12")),
]
