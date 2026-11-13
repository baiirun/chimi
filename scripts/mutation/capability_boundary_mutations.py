"""Mutants of the exclusive-capability boundary: what the compiler checks
(`Channel.Receive<T>` follows `T`, ts-bb95dc) and what the runtime owner token
catches for the flows the compiler does not follow across functions
(task-and-channel-capabilities.md, "Checked at compile time" and "Checked at run
time").

Run: scripts/mutation/mutate.py scripts/mutation/capability_boundary_mutations.py
Each mutant must turn a test red: the typing mutants a capability test in
tests/language.rs, the runtime mutants a `native_backstop_` test in tests/cli.rs.
Results: scripts/mutation/capability_boundary_results.txt.
"""

CRATE = "compiler/chimi"

TYPING = "src/typing.rs"
CHANNELS = "../../runtime/chimi-fiber/src/channel.rs"
SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"
RUNTIME_C = "runtime/native/runtime.c"

RECEIVED = "a_received_value_has_the_capability_class_of_its_payload"
BACKSTOP = "native_backstop_"

MUTATIONS = [
    dict(
        name="Receive<T> is shareable whatever T is",
        file=TYPING,
        edits=[("                StandardType::ProcessSendError | StandardType::ChannelReceive,\n",
                "                StandardType::ProcessSendError,\n")],
        targets=["language"], filter=RECEIVED,
    ),
    dict(
        name="Receive<T> is exclusive whatever T is",
        file=TYPING,
        edits=[("            Some(StandardSymbol::Type(\n"
                "                StandardType::ProcessSendError | StandardType::ChannelReceive,\n"
                "            )) => {\n",
                "            Some(StandardSymbol::Type(StandardType::ChannelReceive)) => {\n"
                "                return ValueProperties::EXCLUSIVE;\n"
                "            }\n"
                "            Some(StandardSymbol::Type(StandardType::ProcessSendError)) => {\n")],
        targets=["language"], filter=RECEIVED,
    ),
    dict(
        name="the wrong-owner receiver check is disabled",
        file=CHANNELS,
        edits=[("    if channel.receiver_owner != Some(task)\n"
                "        || receiver.owner != task\n"
                "        || receiver.token != channel.receiver_token\n"
                "    {\n",
                "    if false {\n")],
        targets=["cli"], filter=BACKSTOP,
    ),
    dict(
        name="a capability violation is not reported to the host",
        file=RUNTIME_C,
        edits=[("    .capability_violation_report = record_capability_violation,\n",
                "    .capability_violation_report = NULL,\n")],
        targets=["cli"], filter=BACKSTOP,
    ),
    dict(
        name="the report names the violating task as the owner",
        file=SCHEDULER,
        edits=[("            owner: owner.map_or(0, TaskId::get),\n",
                "            owner: owner.map_or(0, |_| task.get()),\n")],
        targets=["cli"], filter=BACKSTOP,
    ),
    dict(
        name="a failed transfer is reported as a use",
        file=RUNTIME_C,
        edits=[("  bool transfer = violation->request_kind == CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER ||\n",
                "  bool transfer = false && violation->request_kind == CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER ||\n")],
        targets=["cli"], filter=BACKSTOP,
    ),
]
