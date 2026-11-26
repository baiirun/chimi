"""Mutants of the value cells in Rust (CHI-305 step 5, phase 4, first PR;
runtime/chimi-runtime/src/heap/values.rs): values, controls, value tables,
environments, and the scalar and text builders. The allocation-kind mutants
are caught by the native heap fixture under AddressSanitizer with the heap
check on (native-heap-harness.sh); the others by the language suite, whose
programs run on the native backend.

Run: scripts/mutation/mutate.py scripts/mutation/value_cells_mutations.py
Results: scripts/mutation/value_cells_results.txt.
"""

CRATE = "compiler/chimi"
V = "../../runtime/chimi-runtime/src/heap/values.rs"
HEAP = dict(command=["../../scripts/mutation/native-heap-harness.sh"],
            expect=["check failed", "ERROR: AddressSanitizer", "runtime error:", "heap check failed",
                    "allocation graph is inconsistent"])
LANGUAGE = dict(targets=["language"], filter="")


def heap(name, old, new):
    return dict(name=name, file=V, edits=[(old, new)], **HEAP)


def language(name, old, new):
    return dict(name=name, file=V, edits=[(old, new)], **LANGUAGE)


MUTATIONS = [
    heap("a value keeps the BYTES allocation kind",
         "        let value = allocate_kind::<Value>(1, AllocationKind::VALUE);",
         "        let value = allocate_kind::<Value>(1, AllocationKind::BYTES);"),
    heap("a value table is counted as one value",
         "    unsafe { allocate_kind::<*mut Value>(count as usize, AllocationKind::VALUE_POINTERS) }",
         "    unsafe { allocate_kind::<*mut Value>(count as usize, AllocationKind::VALUE) }"),
    heap("a frame environment is counted as a captured one",
         "        let frame = environment(AllocationKind::FRAME_ENVIRONMENT, local_count, ptr::null_mut(), ptr::null_mut());",
         "        let frame = environment(AllocationKind::ENVIRONMENT, local_count, ptr::null_mut(), ptr::null_mut());"),
    language("a frame copy shares the locals it should copy",
             "        (*frame).locals = if locals { clone_value_slots((*source).locals, count) } else { (*source).locals };",
             "        (*frame).locals = (*source).locals;"),
    language("a frame copy drops the requirements it shares",
             "        } else {\n            (*source).requirements\n        };",
             "        } else {\n            ptr::null_mut()\n        };"),
    language("a captured closure loses its requirements",
             "        (*captured).requirements = clone_value_slots((*source).requirements, requirement_slots());",
             "        (*captured).requirements = new_value_slots(requirement_slots());"),
    language("every 64-bit value is an Int",
             "        let value = new_value(kind);\n        (*value).payload.integer = raw;",
             "        let value = new_value(ValueKind::INTEGER);\n        (*value).payload.integer = raw;"),
    language("a range's end is its start",
             "        (*value).payload.range.end = end;",
             "        (*value).payload.range.end = start;"),
    language("a text copy drops its last byte",
             "            ptr::copy_nonoverlapping(bytes, copy, length);",
             "            ptr::copy_nonoverlapping(bytes, copy, length - 1);"),
    language("a text's length counts its terminator",
             "        (*value).payload.text.length = length;",
             "        (*value).payload.text.length = length + 1;"),
    language("a control keeps no node",
             "        (*control).node = node;",
             "        (*control).node = -1;"),
]
