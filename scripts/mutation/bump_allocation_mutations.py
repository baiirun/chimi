"""Mutants of bump allocation in loop regions (CHI-219, R2 part 2).

Run: scripts/mutation/mutate.py scripts/mutation/bump_allocation_mutations.py
Each mutant must turn a language test or a native cli test red.
The bump itself, the chunk reset and the dead-handle drop are Rust since
CHI-305 step 5 phase 1; their mutants are in young_mutations.py. The
collector, whose forwarding these mutate, is Rust since phase 2
(runtime/chimi-runtime/src/heap/collector.rs).
"""

CRATE = "compiler/chimi"

C = "../../runtime/chimi-runtime/src/heap/collector.rs"

LANGUAGE = dict(targets=["language"], filter=None)
HEAP = dict(targets=["cli"], filter="native_")


def m(name, edits, selection):
    return dict(name=name, file=C, edits=edits, **selection)


LINKS = "        (*scope).links_young = promoted.wrapping_mul(4) > young_bytes;\n"

MUTATIONS = [
 m("a list node's slots are not visited",
   [("                for index in 0..(*node).count as usize {\n                    visit(slots.add(index), kind);\n                }\n", "")],
   LANGUAGE),
 m("a record's fields are not visited",
   [("                    visit(slot!((*fields.add(index)).value), Kind::VALUE);\n", "")],
   LANGUAGE),
 m("forwarding leaves the values a pass moved behind",
   [("    unsafe { each_child(payload, |slot, _| *slot = forward(here, *slot)) }",
     "    unsafe { each_child(payload, |slot, kind| if kind != AllocationKind::VALUE { *slot = forward(here, *slot) }) }")],
   LANGUAGE),
 m("a value moved twice gets two copies",
   [("        if !(*header).next.is_null() {\n            return (*header).next.add(1).cast();\n        }\n", "")], HEAP),
 m("chunks are reset before the survivors move",
   [("        forward_survivors(here, scope, value, control);\n        young::reset_chunks(here, scope, !ending);\n",
     "        young::reset_chunks(here, scope, !ending);\n        forward_survivors(here, scope, value, control);\n")],
   LANGUAGE),
 m("list allocations of the pass are not rewritten",
   [("        forward_marked_list(here, &raw mut (*scope).young);\n", "")], LANGUAGE),
 m("the root keeps pointing into the chunk",
   [("            *value = forward(here, (*value).cast()).cast();\n", "            let _ = forward(here, (*value).cast());\n")],
   LANGUAGE),
 m("a region never links after a pass that mostly survives",
   [(LINKS, "        (*scope).links_young = false;\n        let _ = (promoted, young_bytes);\n")], HEAP),
 m("a region that links never bumps again",
   [(LINKS, "        (*scope).links_young = (*scope).links_young || promoted.wrapping_mul(4) > young_bytes;\n")],
   HEAP),
 m("a control's cause is not visited",
   [("                visit(slot!((*control).cause), Kind::VALUE);\n", "")],
   dict(targets=["cli", "language"], filter=None)),
 m("the counters of the pass are never added",
   [("        young::flush_counts(here, scope);\n        (*here).collecting_scope = scope;",
     "        (*here).collecting_scope = scope;")], HEAP),
]
