"""Mutants of std/channel, std/broadcast, std/watch, `Task.result` and
`Task.cancel` in Rust (CHI-305 step 5, phase 7, part 5): the replies each
request maps to, the channel's capability record, and the route that keeps
`runtime.c`'s waits reaching them.

Run: scripts/mutation/mutate.py scripts/mutation/channels_mutations.py
Results: scripts/mutation/channels_results.txt.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
CHANNELS = "../../runtime/chimi-runtime/src/intrinsics/channels.rs"


def mutant(name, file, edits, targets=("language", "channel_variants", "wait"), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("a send to a closed channel reads as aborted", CHANNELS, [(
        "standard_value(StandardConstructor::CHANNEL_SEND_CLOSED, &[value])",
        "standard_value(StandardConstructor::CHANNEL_SEND_ABORTED, &[value])",
    )], targets=("cli",), filter="native_channel_"),
    mutant("a drained receiver reads as aborted", CHANNELS, [(
        "return success(standard_value(StandardConstructor::CHANNEL_DRAINED, &[]));",
        "return failure(standard_value(StandardConstructor::CHANNEL_ABORTED, &[]));",
    )]),
    mutant("Task.result does not wait", CHANNELS, [(
        "                    Scheduler::RESPONSE_PENDING => continue,\n                    Scheduler::RESPONSE_SUCCEEDED => return task_entry::task_reply",
        "                    Scheduler::RESPONSE_PENDING => return defect(c\"pending\", node),\n                    Scheduler::RESPONSE_SUCCEEDED => return task_entry::task_reply",
    )]),
    mutant("a defected task's result is a request failure", CHANNELS, [(
        "                    Scheduler::RESPONSE_DEFECTED if request.value > 0 => return task_entry::task_reply(request.value),\n",
        "",
    )], targets=("testing", "cli", "task_release", "resources")),
    mutant("a channel's completion and receiver swap", CHANNELS, [(
        "        (*fields).value = capability(ValueKind::CHANNEL_COMPLETION, channel, owner, token);\n        (*fields.add(1)).value = capability(ValueKind::CHANNEL_RECEIVER, channel, owner, token);\n        (*fields.add(2))",
        "        (*fields).value = capability(ValueKind::CHANNEL_RECEIVER, channel, owner, token);\n        (*fields.add(1)).value = capability(ValueKind::CHANNEL_COMPLETION, channel, owner, token);\n        (*fields.add(2))",
    )]),
    mutant("an accepted close or cancel is a defect", CHANNELS, [(
        "if request.status == Scheduler::RESPONSE_OK { success(void_value()) } else { defect(failed, node) }",
        "if request.status != Scheduler::RESPONSE_OK { success(void_value()) } else { defect(failed, node) }",
    )]),
    mutant("a wait's receive and cancel no longer reach Rust", C, [(
        "  if ((intrinsic >= CHIMI_INTRINSIC_TASK_RESULT && intrinsic <= CHIMI_INTRINSIC_CHANNEL_BOUNDED) ||",
        "  if ((intrinsic == CHIMI_INTRINSIC_TASK_RESULT) ||",
    )]),
    mutant("a negative channel capacity is accepted", CHANNELS, [(
        "Intrinsic::CHANNEL_BOUNDED if one(ValueKind::INTEGER) && (*arguments[0]).payload.integer >= 0 =>",
        "Intrinsic::CHANNEL_BOUNDED if one(ValueKind::INTEGER) =>",
    )]),
]
