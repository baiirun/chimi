"""Mutants of the native handle slot table in Rust (CHI-309 part 2,
runtime/chimi-runtime/src/handles.rs), caught by its unit tests. The C side's
use of it (locks, holders, runners, releases after unlock) is mutated in
binding_mutations.py, binding_run_mutations.py and carrier_tables_mutations.py.

Run: scripts/mutation/mutate.py scripts/mutation/handle_slots_mutations.py
Results: scripts/mutation/handle_slots_results.txt.
"""

CRATE = "runtime/chimi-runtime"
HANDLES = "src/handles.rs"


def m(name, old, new):
    return dict(name=name, file=HANDLES, edits=[(old, new)], targets=["lib"], filter="handles::")


MUTATIONS = [
    m("a freed slot is never reused",
      "            *entry = Slot::Free { generation, next };\n            self.free = slot;",
      "            *entry = Slot::Free { generation, next };"),
    m("reusing a slot drops the rest of the free list",
      "        if slot == self.free {\n            self.free = next;\n        }",
      "        if slot == self.free {\n            self.free = 0;\n        }"),
    m("a slot at the last generation is reused",
      "        if generation == u32::MAX {",
      "        if false {"),
    m("the limit is ignored",
      ".filter(|slot| *slot < LIMIT)?;",
      "?;"),
    m("a reused slot starts its generation over",
      "        *entry = Slot::Open(Live { kind, state, generation, users: 0, runner: None });",
      "        *entry = Slot::Open(Live { kind, state, generation: 0, users: 0, runner: None });\n        let _ = generation;"),
    m("a close of another kind closes it",
      "        if kind.is_some_and(|kind| kind != live.kind) {",
      "        if false && kind.is_some_and(|kind| kind != live.kind) {"),
    m("holding a closed handle succeeds",
      "    pub fn hold(&mut self, slot: u32, generation: u32) -> bool {\n        match self.open_entry(slot, generation) {",
      "    pub fn hold(&mut self, slot: u32, generation: u32) -> bool {\n        let _ = generation;\n        match self.live(slot) {"),
    m("letting go of an open handle releases it",
      "            Slot::Open(live) => {\n                live.users = live.users.saturating_sub(1);\n                None\n            }",
      "            Slot::Open(live) => {\n                live.users = live.users.saturating_sub(1);\n                let live = *live;\n                self.free(slot, live.generation);\n                Some(Release { kind: live.kind, state: live.state })\n            }"),
    m("the end leaves an open handle's generation",
      "            Slot::Open(live) => (*live, live.generation.saturating_add(1)),",
      "            Slot::Open(live) => (*live, live.generation),"),
    m("the end skips a handle closed while held",
      "            Slot::Closing(live) => (*live, live.generation),\n            Slot::Free { .. } | Slot::Retired => return None,",
      "            Slot::Closing(_) | Slot::Free { .. } | Slot::Retired => return None,"),
    m("a runner is not kept",
      "            live.runner = runner;",
      "            let _ = runner;"),
]
