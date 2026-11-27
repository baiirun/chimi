"""Mutants of G28 and CHI-213: handoff starts, and capabilities carried in a
Process.send message (docs/specification/process-and-supervisor-static-contracts.md,
"Handoff starts", and docs/specification/task-and-channel-capabilities.md).

Run: scripts/mutation/mutate.py scripts/mutation/g28_mutations.py
Results: scripts/mutation/g28_results.txt.
"""

CRATE = "compiler/chimi"

PROCESS = "../../runtime/chimi-fiber/src/scheduler/process.rs"
PROCESS_ABI = "../../runtime/chimi-fiber/src/scheduler/process_abi.rs"
RUNTIME_C = "runtime/native/runtime.c"
# The supervisor starts marshal their specs and policies in Rust since
# CHI-305 step 5, phase 7.
STARTS = "../../runtime/chimi-runtime/src/intrinsics/process_starts.rs"


def lab(scenario):
    return dict(
        command=["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
                 "--scenario", scenario],
        expect=[scenario],
    )


MUTATIONS = [
    dict(name="a received message's capabilities stay with the sender", file=PROCESS,
         edits=[("            self.channels.deliver_parcel(message as usize, task);\n        }\n        receipt",
                 "        }\n        receipt")],
         **lab("CB09")),
    dict(name="an unread message is released without aborting its channels", file=PROCESS,
         edits=[("Entry::Message(message) => self.discard_message(message),",
                 "Entry::Message(message) => self.release_message(message),")],
         **lab("CB10")),
    dict(name="an untaken argument is released without aborting its channels", file=PROCESS,
         edits=[("        if let Some(argument) = record.argument {\n            self.discard_message(argument);",
                 "        if let Some(argument) = record.argument {\n            self.release_message(argument);")],
         **lab("CB11")),
    dict(name="the start argument's capabilities are never put in flight", file=PROCESS,
         edits=[("        self.channels\n            .dispatch_carried(sender, argument as usize, carried);\n", "")],
         **lab("MB19")),
    dict(name="taking the argument leaves its capabilities in flight", file=PROCESS,
         edits=[("        self.channels.deliver_parcel(argument as usize, task);\n        Some(argument)",
                 "        Some(argument)")],
         **lab("MB19")),
    dict(name="a handoff member restarts", file=PROCESS,
         edits=[("let membership = self.start_process(supervisor, spec, Restart::Never)?;",
                 "let membership = self.start_process(supervisor, spec, Restart::OnDefect)?;")],
         **lab("MB20")),
    dict(name="a stopped supervisor reports a full one", file=STARTS,
         edits=[("failure(standard_value(StandardConstructor::HANDOFF_STOPPED, &[arguments[2]]))",
                 "failure(standard_value(StandardConstructor::HANDOFF_CAPACITY_REACHED, &[arguments[2]]))")],
         **lab("MB21")),
    dict(name="the argument may be used after start_handoff", file="src/capability.rs",
         edits=[("                    Some(StandardFunction::SupervisorStartHandoff) => {\n                        if let Some((argument, start)) = arguments.get(2) {",
                 "                    Some(StandardFunction::SupervisorStartHandoff) => {\n                        if let Some((argument, start)) = arguments.get(9) {")],
         **lab("SF29")),
    dict(name="a handoff body may capture a capability", file="src/capability.rs",
         edits=[("                        function @ (StandardFunction::ProcessSpec\n                        | StandardFunction::ProcessHandoffSpec),",
                 "                        function @ StandardFunction::ProcessSpec,")],
         **lab("SF30")),
    dict(name="the start argument need not be transferable", file="src/typing/process.rs",
         edits=[("                        self.check_start_argument_transferable(module_id, body.range(), start);",
                 "                        let _ = start;")],
         **lab("SF31")),
]
