"""Collector bugs the heap check (chimi-runtime heap/verify.rs, CHI-305 step
5 phase 0) must catch on its own: the native heap fixture, whose values are
lists of integers, runs with the check
on and without AddressSanitizer, so a mutant is killed only when the check
reports it. That is the guard the collector keeps once it moves to Rust,
which the sanitizer does not instrument.

Run: scripts/mutation/mutate.py scripts/mutation/heap_verify_mutations.py
Results: scripts/mutation/heap_verify_results.txt.
"""

CRATE = "compiler/chimi"
R = "runtime/native/runtime.c"
CHECK = dict(command=["../../scripts/mutation/heap-verify-harness.sh"], expect=["heap check failed"])


def m(name, old, new, **overrides):
    return dict(name=name, file=R, edits=[(old, new)], **{**CHECK, **overrides})


MUTATIONS = [
    m("a full pass frees a marked survivor",
      "    if (header->mark_epoch == carrier->collecting_epoch) {\n      link = &header->next;\n      continue;\n    }",
      "    if (header->mark_epoch == carrier->collecting_epoch && header->kind != ALLOCATION_LIST_NODE) {\n      link = &header->next;\n      continue;\n    }"),
    m("a sweep leaves the survivor counts behind",
      "    *link = header->next;\n    area->bytes -= header->bytes;",
      "    *link = header->next;"),
    m("a pass frees a marked young allocation",
      "    if (header->mark_epoch == carrier->collecting_epoch) {\n      area_link(&scope->survivors, header);",
      "    if (header->mark_epoch == carrier->collecting_epoch && header->kind != ALLOCATION_LIST_NODE) {\n      area_link(&scope->survivors, header);"),
    dict(name="marking skips a list node's values, unchecked", file=R, **CHECK, edits=[
        ("      for (uint32_t index = 0; index < node->count; ++index) mark_push(node->slots[index], kind);",
         "      for (uint32_t index = 0; index < node->count && kind != ALLOCATION_VALUE; ++index)\n"
         "        mark_push(node->slots[index], kind);"),
        # The collector's own forwarding check would meet the unmarked chunk
        # value first; without it the heap check has to catch the bug.
        ("  if (header->mark_epoch != here->collecting_epoch) allocation_graph_inconsistent();\n"
         "  size_t total = sizeof(*header) + header->bytes;",
         "  size_t total = sizeof(*header) + header->bytes;"),
    ]),
    m("a full pass keeps unmarked list nodes",
      "    if (header->mark_epoch == carrier->collecting_epoch) {\n      link = &header->next;\n      continue;\n    }",
      "    if (header->mark_epoch == carrier->collecting_epoch || header->kind == ALLOCATION_LIST_NODE) {\n"
      "      link = &header->next;\n      continue;\n    }"),
    m("young chunk survivors are not moved",
      "  forward_survivors(scope, value, control);\n  reset_young_chunks(scope, !ending);",
      "  reset_young_chunks(scope, !ending);"),
    m("adopted areas are not promoted",
      "  while (scope->adopted) {\n    AllocationArea *area = scope->adopted;\n    scope->adopted = area->next;\n    promote_marked(scope, area);\n    area_free(area);\n  }",
      ""),
]
