"""Mutants of the list vector's builders in Rust (CHI-305 step 5, phase 4,
second PR; runtime/chimi-runtime/src/heap/list.rs): push, set, building
from values, append to a window, and slicing with compaction. They are
caught by the differential list fuzz (tests/native_list_fuzz.rs, built with
AddressSanitizer) and the persistent-vector fixture
(native_list_is_a_persistent_vector in tests/cli.rs). The slice's
shared-constant mutants are in shared_constant_mutations.py.

Run: scripts/mutation/mutate.py scripts/mutation/list_vector_mutations.py
Results: scripts/mutation/list_vector_results.txt.
"""

CRATE = "compiler/chimi"
L = "../../runtime/chimi-runtime/src/heap/list.rs"


def fuzz(name, old, new):
    return dict(name=name, file=L, edits=[(old, new)], targets=["native_list_fuzz"], filter="")


def vector(name, old, new):
    return dict(name=name, file=L, edits=[(old, new)], targets=["cli"],
                filter="native_list_is_a_persistent_vector")


# The fuzz compares values only, so a mutant that keeps values right but
# retains or copies too much goes to the persistent-vector fixture, which
# bounds live bytes and the cost of dropping elements.


MUTATIONS = [
    fuzz("list compaction allocates in the current area, not the list's own",
         "            (*list).payload.list.vector = copy_window(here, list, 0, window.length, area);",
         "            (*list).payload.list.vector = copy_window(here, list, 0, window.length, ptr::null_mut());"),
    fuzz("a push writes over the tail's last element",
         "            *slots(next).add(tail_length as usize) = item.cast();",
         "            *slots(next).add(tail_length.saturating_sub(1) as usize) = item.cast();"),
    fuzz("a grown root drops its old subtree",
         "            *slots(grown) = vector.root.cast();",
         "            *slots(grown) = new_path((*vector.root).height, vector.tail).cast();"),
    fuzz("a copied path has one slot too few",
         "            let copy = node_copy(source, slot + 1);",
         "            let copy = node_copy(source, slot.max(1));"),
    fuzz("a set writes the shared node instead of a copy",
         "            let copy = node_copy(source, (*source).count);",
         "            let copy = source;"),
    fuzz("an append to a short window pushes past the vector's end",
         "        let vector = if end == window.vector.length { push(window.vector, item) } else { set(window.vector, end, item) };",
         "        let vector = push(window.vector, item);"),
    vector("a slice never compacts",
         "        if retains_too_much(length, window.vector.length) {",
         "        if false {"),
    # Not listed: building a 33-64 element vector with its one leaf as the
    # root. Every read, write, push and walk handles that shape, so only one
    # 16-byte node differs.
    fuzz("a rebuilt vector's inner nodes all have height 1",
           "            height += 1;\n",
           "            height = 1;\n"),
    vector("a rebuilt vector's leaves start one element late",
           "            ptr::copy_nonoverlapping(values.add(index << LIST_BITS).cast(), slots(leaf), WIDTH as usize);",
           "            ptr::copy_nonoverlapping(values.add((index << LIST_BITS) + 1).cast(), slots(leaf), WIDTH as usize - 1);"),
]
