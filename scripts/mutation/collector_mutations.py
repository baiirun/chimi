"""Mutants of the collector's pass choice and marking in Rust (CHI-305 step 5,
phase 2; runtime/chimi-runtime/src/heap/collector.rs), caught by the native
heap fixture under AddressSanitizer (native-heap-harness.sh), which drives
region passes directly and checks what each one keeps and what it costs.
They were runtime.c mutants in compiler/chimi/tests/fixtures/native-heap-mutants.py
until the collector moved. The collector's other mutants were retargeted in
place: bump_allocation_mutations.py (forwarding and chunk moves),
heap_verify_mutations.py (bugs the heap check alone must catch),
deep_value_mutations.py (the full-pass choice) and
carrier_measure_mutations.py (mark epochs).

Run: scripts/mutation/mutate.py scripts/mutation/collector_mutations.py
Results: scripts/mutation/collector_results.txt.
"""

CRATE = "compiler/chimi"
C = "../../runtime/chimi-runtime/src/heap/collector.rs"
HEAP = dict(command=["../../scripts/mutation/native-heap-harness.sh"],
            expect=["check failed", "ERROR: AddressSanitizer", "runtime error:", "heap check failed"])
FULL = ("        let full = (*scope).has_baseline\n"
        "            && survivors >= baseline.wrapping_mul(2)\n"
        "            && survivors.wrapping_sub(baseline) >= REGION_FULL_MIN_GROWTH;\n")


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], **HEAP)


MUTATIONS = [
    m("a minor pass rescans the old survivors",
      "        if (*area).role == AreaRole::SURVIVOR && !(*here).collecting_full {\n            return;\n        }\n",
      ""),
    m("every pass after the first is full", FULL,
      "        let full = (*scope).has_baseline;\n        let _ = (survivors, baseline);\n"),
    m("a full pass needs no minimum growth", FULL,
      "        let full = (*scope).has_baseline && survivors >= baseline.wrapping_mul(2);\n"),
    m("no pass is ever full", FULL, "        let full = false;\n        let _ = (survivors, baseline);\n"),
    m("a region's end drops its survivors",
      "        let status = sweep(here, scope, value, control, true, true);\n        young::scope_end(scope);\n",
      "        let status = sweep(here, scope, value, control, true, true);\n"
      "        let mut header = (*scope).survivors.allocations;\n"
      "        while !header.is_null() {\n"
      "            let next = (*header).next;\n"
      "            release(here, header);\n"
      "            header = next;\n"
      "        }\n"
      "        (*scope).survivors.allocations = ptr::null_mut();\n"
      "        (*scope).survivors.bytes = 0;\n"
      "        (*scope).survivors.count = 0;\n"
      "        (*scope).live_bytes = 0;\n"
      "        young::scope_end(scope);\n"),
    m("a pass ignores a pointer into another heap",
      "            if (*area).heap != (*scope).heap && (*area).role != AreaRole::STATIC {\n"
      "                (*here).collecting_foreign = true;\n            }\n",
      ""),
    m("a pointer of the wrong kind is followed",
      "        if (*header).kind != kind {\n            chimi_runtime_allocation_graph_inconsistent();\n        }\n",
      ""),
]
