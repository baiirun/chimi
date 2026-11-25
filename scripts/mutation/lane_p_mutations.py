"""Mutants of the Lane P process hardening (CHI-183, CHI-184, CHI-185, CHI-190):
capabilities that follow the value holding them, `Channel.SendError<T>` following
`T`, process defects named in the root outcome under any trace mode, the PG05
lab flags, and supervisor time.

Run: scripts/mutation/mutate.py scripts/mutation/lane_p_mutations.py
Results: scripts/mutation/lane_p_results.txt.
"""

CRATE = "compiler/chimi"

TYPING = "src/typing.rs"
CHANNELS = "../../runtime/chimi-fiber/src/channel.rs"
SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"
PROCESS = "../../runtime/chimi-fiber/src/scheduler/process.rs"
RUNTIME_C = "runtime/native/runtime.c"
LAB_BIN = "src/bin/process_lab.rs"

HAND_BACK = "native_a_receiver_"
STALE = "native_backstop_a_stale_copy"

MUTATIONS = [
    # CHI-184
    dict(
        name="SendError<T> is shareable whatever T is",
        file=TYPING,
        edits=[("                | StandardType::ChannelSendError\n"
                "                | StandardType::ChannelReceive,\n",
                "                | StandardType::ChannelReceive,\n")],
        targets=["language"], filter="a_refused_send_has_the_capability_class_of_its_payload",
    ),
    # CHI-185: the channel route
    dict(
        name="a sent message carries no capabilities",
        file=RUNTIME_C,
        edits=[("          .context = first && capabilities.count ? &capabilities : NULL,\n",
                "          .context = NULL,\n")],
        targets=["cli"], filter=HAND_BACK,
    ),
    dict(
        name="the message's copies keep the sender's token",
        file=RUNTIME_C,
        edits=[("  ++value->as.capability.token;\n", "")],
        targets=["cli"], filter=HAND_BACK,
    ),
    dict(
        name="a delivered capability stays in flight",
        file=CHANNELS,
        edits=[("                        self.land(item, receiver, None);\n", "")],
        targets=["cli"], filter=HAND_BACK,
    ),
    dict(
        name="a refused send keeps the in-flight token",
        file=CHANNELS,
        edits=[("                        self.land(item, parcel.sender, Some(item.token));\n",
                "                        self.land(item, parcel.sender, None);\n")],
        targets=["cli"], filter=HAND_BACK,
    ),
    dict(
        name="a discarded message leaves its capabilities' channels open",
        file=CHANNELS,
        edits=[("        if should_abort {\n            self.abort_channel(item.channel, None, ChannelEventKind::Aborted)\n",
                "        if false {\n            self.abort_channel(item.channel, None, ChannelEventKind::Aborted)\n")],
        targets=["cli"], filter=HAND_BACK,
    ),
    dict(
        name="an in-flight capability validates as the sender's",
        file=CHANNELS,
        edits=[("    if channel.receiver_parcel.is_some() {\n"
                "        return Err(ChannelError::CapabilityInFlight {\n",
                "    if false {\n"
                "        return Err(ChannelError::CapabilityInFlight {\n")],
        targets=["cli"], filter=HAND_BACK,
    ),
    dict(
        name="a carried capability is not validated",
        file=CHANNELS,
        edits=[("        let carried = self.validate_carried(task, carried)?;\n",
                "        let carried = carried.to_vec();\n")],
        targets=["cli"], filter=STALE,
    ),
    # CHI-185: the Task.result route
    dict(
        name="a child's result keeps its capabilities",
        file=RUNTIME_C,
        edits=[("  if ((control->kind == CONTROL_SUCCESS || control->kind == CONTROL_FAILURE) && control->value) {\n",
                "  if (false) {\n")],
        targets=["cli"], filter=HAND_BACK,
    ),
    dict(
        name="the child hands its result to itself",
        file=SCHEDULER,
        edits=[("                        // this task (`include/chimi_fiber.h`).\n"
                "                        request.owner = owner.get();\n",
                "                        // this task (`include/chimi_fiber.h`).\n"
                "                        request.owner = task.get();\n")],
        targets=["cli"], filter=HAND_BACK,
    ),
    # CHI-183
    dict(
        name="the scheduler reports no process defect",
        file=PROCESS,
        edits=[("        if cause == TerminalCause::Defected\n"
                "            && let Some((report, context)) = self.kernel.defect_report\n",
                "        if false\n"
                "            && let Some((report, context)) = self.kernel.defect_report\n")],
        command=["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
                 "--scenario", "OB08"],
        expect=["OB08"],
    ),
    dict(
        name="a stack overflow is reported as the body's own message",
        file=RUNTIME_C,
        edits=[("  if (termination->stack_overflow) {\n", "  if (false) {\n")],
        command=["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
                 "--scenario", "OB08"],
        expect=["OB08"],
    ),
    dict(
        name="a body's defect is remembered only while tracing",
        file=RUNTIME_C,
        edits=[("      chimi_fiber_report_defect(control->message);\n",
                "      if (!runtime->silent) chimi_fiber_report_defect(control->message);\n")],
        command=["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
                 "--scenario", "OB08"],
        expect=["OB08"],
    ),
    # CHI-190
    dict(
        name="--baseline without performance is ignored again",
        file=LAB_BIN,
        edits=[("    if baseline.is_some() && !measures_gates {\n",
                "    if false && baseline.is_some() && !measures_gates {\n")],
        targets=["process_lab"], filter="a_baseline_or_injection_without_performance_is_an_error",
    ),
]
