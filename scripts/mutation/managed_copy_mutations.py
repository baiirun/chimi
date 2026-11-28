"""Mutants of the managed-handle backstop for broadcast and watch (CHI-240).

Run: scripts/mutation/mutate.py scripts/mutation/managed_copy_mutations.py
Each mutant must turn the channel-variant tests red.
Results: scripts/mutation/managed_copy_results.txt.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
PROCESS_KERNEL = "../../runtime/chimi-runtime/src/intrinsics/process_kernel.rs"
CHANNELS = "../../runtime/chimi-runtime/src/intrinsics/channels.rs"
CAPABILITIES = "../../runtime/chimi-runtime/src/intrinsics/capabilities.rs"


def variant(name, edits, file=RUNTIME):
    return dict(name=name, file=file, edits=edits, targets=["channel_variants"], filter="managed")


MUTATIONS = [
    variant("managed handles are not counted", [(
        "                    transfer.managed_count += 1;\n",
        "")], CAPABILITIES),
    variant("broadcast publishes a managed handle", [(
        "        if carried.managed != 0 && intrinsic != Intrinsic::CHANNEL_SEND {",
        "        if carried.managed != 0 && false {")], CHANNELS),
    variant("a watch publishes a managed handle", [(
        "        if carried.managed != 0 && intrinsic != Intrinsic::CHANNEL_SEND {",
        "        if carried.managed != 0 && intrinsic == Intrinsic::BROADCAST_PUBLISH {")], CHANNELS),
    variant("an ordinary channel refuses a managed handle too", [(
        "        if carried.managed != 0 && intrinsic != Intrinsic::CHANNEL_SEND {",
        "        if carried.managed != 0 {")], CHANNELS),
    variant("a watch starts with a managed handle", [(
        "        if count != 0 || managed != 0 {",
        "        if count != 0 {")], CHANNELS),
    variant("a second remove keeps the generic message", [(
        "                b\"; it was removed already, through this handle or a copy of it, or the supervisor has stopped\",",
        "                b\"; scheduler request failed\",")], file=PROCESS_KERNEL),
]
