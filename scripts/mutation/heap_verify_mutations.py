"""Collector bugs the heap check (chimi-runtime heap/verify.rs, CHI-305 step
5 phase 0) must catch on its own: the native heap fixture runs with the check
on and without AddressSanitizer, so a mutant is killed only when the check
reports it. That is the guard the collector keeps once it moves to Rust,
which the sanitizer does not instrument.

Run: scripts/mutation/mutate.py scripts/mutation/heap_verify_mutations.py
Results: scripts/mutation/heap_verify_results.txt.
"""

CRATE = "compiler/chimi"
R = "runtime/native/runtime.c"
CHECK = dict(command=["../../scripts/mutation/heap-verify-harness.sh"], expect=["heap check failed"])


def m(name, old, new):
    return dict(name=name, file=R, edits=[(old, new)], **CHECK)


MUTATIONS = [
    m("a full pass frees a marked survivor",
      "    if (header->mark_epoch == carrier->collecting_epoch) {\n      link = &header->next;\n      continue;\n    }",
      "    if (header->mark_epoch == carrier->collecting_epoch && header->kind != ALLOCATION_MAP_NODE) {\n      link = &header->next;\n      continue;\n    }"),
    m("a sweep leaves the survivor counts behind",
      "    *link = header->next;\n    area->bytes -= header->bytes;",
      "    *link = header->next;"),
    m("a pass frees a marked young allocation",
      "    if (header->mark_epoch == carrier->collecting_epoch) {\n      area_link(&scope->survivors, header);",
      "    if (header->mark_epoch == carrier->collecting_epoch && header->kind != ALLOCATION_FIELDS) {\n      area_link(&scope->survivors, header);"),
    m("marking skips a map node's children",
      "      mark_push(node->left, ALLOCATION_MAP_NODE);\n      mark_push(node->right, ALLOCATION_MAP_NODE);\n      break;\n    }\n    case ALLOCATION_CONTROL:\n      mark_push(",
      "      break;\n    }\n    case ALLOCATION_CONTROL:\n      mark_push("),
    m("young chunk survivors are not moved",
      "  forward_survivors(scope, value, control);\n  reset_young_chunks(scope, !ending);",
      "  reset_young_chunks(scope, !ending);"),
    m("adopted areas are not promoted",
      "  while (scope->adopted) {\n    AllocationArea *area = scope->adopted;\n    scope->adopted = area->next;\n    promote_marked(scope, area);\n    area_free(area);\n  }",
      ""),
]
