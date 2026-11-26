"""Collector bugs the heap check (chimi-runtime heap/verify.rs, CHI-305 step
5 phase 0) must catch on its own: the native heap fixture, whose values are
lists of integers, runs with the check
on and without AddressSanitizer, so a mutant is killed only when the check
reports it. That is the guard the collector keeps now that it is Rust (CHI-305 step 5
phase 2, runtime/chimi-runtime/src/heap/collector.rs), which the sanitizer
does not instrument.

Run: scripts/mutation/mutate.py scripts/mutation/heap_verify_mutations.py
Results: scripts/mutation/heap_verify_results.txt.
"""

CRATE = "compiler/chimi"
C = "../../runtime/chimi-runtime/src/heap/collector.rs"
CHECK = dict(command=["../../scripts/mutation/heap-verify-harness.sh"], expect=["heap check failed"])

SURVIVOR_KEPT = ("            if (*header).mark_epoch == (*here).collecting_epoch {\n"
                 "                link = &raw mut (*header).next;")


def m(name, old, new, **overrides):
    return dict(name=name, file=C, edits=[(old, new)], **{**CHECK, **overrides})


MUTATIONS = [
    m("a full pass frees a marked survivor", SURVIVOR_KEPT,
      "            if (*header).mark_epoch == (*here).collecting_epoch && (*header).kind != AllocationKind::LIST_NODE {\n"
      "                link = &raw mut (*header).next;"),
    m("a sweep leaves the survivor counts behind",
      "            (*area).bytes = (*area).bytes.wrapping_sub((*header).bytes);\n", ""),
    m("a pass frees a marked young allocation",
      "            if (*header).mark_epoch == (*here).collecting_epoch {\n                area_link(",
      "            if (*header).mark_epoch == (*here).collecting_epoch && (*header).kind != AllocationKind::LIST_NODE {\n"
      "                area_link("),
    dict(name="marking skips a list node's values, unchecked", file=C, **CHECK, edits=[
        ("            each_child(payload, |slot, kind| mark(here, *slot, kind));",
         "            let list = (*header_of(payload)).kind == AllocationKind::LIST_NODE;\n"
         "            each_child(payload, |slot, kind| if !(list && kind == AllocationKind::VALUE) { mark(here, *slot, kind) });"),
        # The collector's own forwarding check would meet the unmarked chunk
        # value first; without it the heap check has to catch the bug.
        ("        if (*header).mark_epoch != (*here).collecting_epoch {\n"
         "            chimi_runtime_allocation_graph_inconsistent();\n        }\n        moved(",
         "        moved("),
    ]),
    m("a full pass keeps unmarked list nodes", SURVIVOR_KEPT,
      "            if (*header).mark_epoch == (*here).collecting_epoch || (*header).kind == AllocationKind::LIST_NODE {\n"
      "                link = &raw mut (*header).next;"),
    m("young chunk survivors are not moved",
      "        forward_survivors(here, scope, value, control);\n        young::reset_chunks",
      "        young::reset_chunks"),
    m("adopted areas are not promoted",
      "        while !(*scope).adopted.is_null() {\n            let area = (*scope).adopted;\n"
      "            (*scope).adopted = (*area).next;\n            promote_marked(here, scope, area);\n"
      "            area_free(here, area);\n        }\n",
      ""),
]
