"""Mutants of notices that carry capabilities (CHI-227).

Run: scripts/mutation/mutate.py scripts/mutation/notice_capability_mutations.py
Results: scripts/mutation/notice_capability_results.txt.
"""

CRATE = "compiler/chimi"

MONITOR = "../../runtime/chimi-fiber/src/scheduler/process/monitor.rs"
PROCESS = "../../runtime/chimi-fiber/src/scheduler/process.rs"
PROCESS_ABI = "../../runtime/chimi-fiber/src/scheduler/process_abi.rs"
RUNTIME_C = "runtime/native/runtime.c"


def lab(scenario):
    return dict(
        command=["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
                 "heap", "--scenario", scenario],
        expect=[scenario],
    )


MUTATIONS = [
    dict(name="a notice's capabilities stay with the watcher", file=PROCESS_ABI, edits=[(
        "                self.channels.dispatch_carried(owner, input.message as usize, &carried);\n",
        "")], **lab("CB13")),
    dict(name="the host sends no capabilities with a notice", file=RUNTIME_C, edits=[(
        "        .carried = capabilities.count ? &capabilities : NULL,\n      };\n      ChimiFiberCommand command = scheduler_request(&request);\n      free(carried.carried);\n      if (request.status != CHIMI_RESPONSE_OK) fragment_free(notice);",
        "      };\n      ChimiFiberCommand command = scheduler_request(&request);\n      free(carried.carried);\n      if (request.status != CHIMI_RESPONSE_OK) fragment_free(notice);")], **lab("CB13")),
    dict(name="unmonitoring a pending monitor keeps its capabilities in flight", file=MONITOR, edits=[(
        "            self.unlink(&record, monitor);\n            self.discard_message(record.handle);\n            return;",
        "            self.unlink(&record, monitor);\n            self.release_message(record.handle);\n            return;")], **lab("CB13")),
    dict(name="a received notice keeps its capabilities in flight", file=PROCESS, edits=[(
        "            Receipt::Notice(notice) => self.channels.deliver_parcel(notice.handle as usize, task),\n",
        "")], **lab("CB14")),
]
