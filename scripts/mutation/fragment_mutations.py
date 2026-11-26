"""Mutants of fragment copies and the heaps' lifecycle in Rust (CHI-305 step 5,
phase 3; runtime/chimi-runtime/src/heap/fragment.rs), caught by the native
heap fixture under AddressSanitizer (native-heap-harness.sh). They were
runtime.c mutants in compiler/chimi/tests/fixtures/native-heap-mutants.py
until the code moved; the process lab's fragment mutants were retargeted in
place (process_heap_mutations.py, process_u5_mutations.py), as was the heap
owner counter's in carrier_threads_mutations.py.

Run: scripts/mutation/mutate.py scripts/mutation/fragment_mutations.py
Results: scripts/mutation/fragment_results.txt.
"""

CRATE = "compiler/chimi"
F = "../../runtime/chimi-runtime/src/heap/fragment.rs"
HEAP = dict(command=["../../scripts/mutation/native-heap-harness.sh"],
            expect=["check failed", "ERROR: AddressSanitizer", "runtime error:", "heap check failed"])
WINDOW = ("        for index in 0..length {\n"
          "            let item = chimi_runtime_list_get_v1(list, index);\n"
          "            values.push(forward(here, copy, item.cast(), AllocationKind::VALUE).cast::<Value>());\n"
          "        }\n")


def m(name, old, new):
    return dict(name=name, file=F, edits=[(old, new)], **HEAP)


MUTATIONS = [
    m("adopted messages join the heap's base area, not the innermost region",
      "        if scope.is_null() {\n            (*area).role = AreaRole::BASE;",
      "        if true {\n            (*area).role = AreaRole::BASE;"),
    m("a copy follows a pointer into another heap",
      "        if !copy.owns(area) {\n            copy.foreign(area);\n            return ptr::null_mut();\n        }\n",
      ""),
    m("a copy keeps no forwarding table",
      "        let existing = copy.table.find(pointer);\n        if !existing.is_null() {\n            return existing;\n        }\n",
      ""),
    m("a window's copy copies the whole vector", WINDOW,
      "        let window = (*payload).length;\n"
      "        (*payload).start = 0;\n"
      "        (*payload).length = source.length;\n"
      "        for index in 0..source.length {\n"
      "            let item = chimi_runtime_list_get_v1(list, index);\n"
      "            values.push(forward(here, copy, item.cast(), AllocationKind::VALUE).cast::<Value>());\n"
      "        }\n"
      "        (*payload).length = window;\n"
      "        let length = source.length;\n"),
    m("destroying a heap skips its adopted fragments",
      "        while !(*heap).adopted.is_null() {\n            let area = (*heap).adopted;\n"
      "            (*heap).adopted = (*area).next;\n            area_release_all(here, area);\n"
      "            area_free(here, area);\n        }\n",
      "        (*heap).adopted = ptr::null_mut();\n"),
]
