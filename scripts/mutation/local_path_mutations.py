"""Mutants of R4 step 4e-2: the local path (scheduler/local.rs, the local
process operations in scheduler/process.rs, and the registry's
send_local and receive_local).

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/local_path_mutations.py
Results: scripts/mutation/local_path_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def m(name, old, new, file="src/scheduler/local.rs"):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter="")


MUTATIONS = [
    m("a traced step goes local",
      "            || kernel.config.trace_mode != super::TraceMode::Off\n",
      ""),
    m("a task inside a scope completes locally",
      "            || record.io_wait.is_some()\n            || !record.scopes.is_empty()\n",
      "            || record.io_wait.is_some()\n"),
    m("an escalated park keeps its park number",
      "            record.park = park;\n",
      ""),
    m("released tasks wait past a local step",
      "        if !kernel.released_terminal.is_empty()\n            ||",
      "        if false\n            ||"),
    m("a job waits behind a local resume",
      "        if shard.has_jobs() {\n            return Err(Escalate);\n        }",
      ""),
    m("a task inside a scope resumes locally",
      "            || record.command != ChimiFiberCommand::Resume\n            || !record.scopes.is_empty()\n",
      "            || record.command != ChimiFiberCommand::Resume\n"),
    m("a channel sender is woken before its receiver",
      "        if let Some(receiver) = woken {\n            self.wake(receiver, false);\n        }",
      "        if let Some(receiver) = woken {\n            self.wake(receiver, true);\n        }"),
    m("a pending channel send reports success",
      "            SendResult::Pending => super::RESPONSE_PENDING,",
      "            SendResult::Pending => super::RESPONSE_OK,"),
    m("a process sender resumes last",
      "        unsafe { (*request).status = super::RESPONSE_OK };\n        self.wake(owner, true);",
      "        unsafe { (*request).status = super::RESPONSE_OK };\n        self.wake(owner, false);",
      file="src/scheduler/process.rs"),
    m("a send leaves its receiver asleep",
      "        target.receiving = false;\n        if let Some(task) = woken {\n            self.wake(task, false);\n        }",
      "        target.receiving = false;\n        let _ = woken;",
      file="src/scheduler/process.rs"),
    m("an empty mailbox does not wait",
      "                record.receiving = true;\n                // SAFETY: as above.",
      "                // SAFETY: as above.",
      file="src/scheduler/process.rs"),
    m("a handed value leaves its receiver receiving",
      "            *slot = Some(Operation::ReceiveReady(ReceiveResult::Received(value)));\n            (SendResult::Accepted, Some(receiver))",
      "            let _ = slot;\n            (SendResult::Accepted, Some(receiver))",
      file="src/channel.rs"),
    m("a collected receive result stays",
      "                let Some(Operation::ReceiveReady(result)) = slot.take() else {",
      "                let Some(Operation::ReceiveReady(result)) = *slot else {",
      file="src/channel.rs"),
    m("a full queue takes the value",
      "        } else if channel.queue.len() < channel.capacity {\n            channel.queue.push_back(value);",
      "        } else if channel.queue.len() <= channel.capacity {\n            channel.queue.push_back(value);",
      file="src/channel.rs"),
]
