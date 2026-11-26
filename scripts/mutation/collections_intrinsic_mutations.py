"""Mutants of the std/map and std/set intrinsics in Rust and the standard
values they build (CHI-305 step 5, phase 5, first PR;
runtime/chimi-runtime/src/intrinsics/collections.rs and standard.rs). They
are caught by the std map, set and option checks (std_modules) and the
language suite. The reduce-order mutant is in std2_map_set_mutations.py.

Run: scripts/mutation/mutate.py scripts/mutation/collections_intrinsic_mutations.py
Results: scripts/mutation/collections_intrinsic_results.txt.
"""

CRATE = "compiler/chimi"
INTRINSICS = "../../runtime/chimi-runtime/src/intrinsics/"
COLLECTIONS = INTRINSICS + "collections.rs"
STANDARD = INTRINSICS + "standard.rs"


def programs(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["std_modules"], filter="standard_")


MUTATIONS = [
    programs("Map.get finds nothing", COLLECTIONS,
             "(Intrinsic::MAP_GET, 2) => option_value(map::get(tree, arguments[1])),",
             "(Intrinsic::MAP_GET, 2) => option_value(core::ptr::null_mut()),"),
    programs("Map.remove of a held key returns the old map", COLLECTIONS,
             "success(if removed == tree { first } else { pointer_value(ValueKind::MAP, removed.cast()) })",
             "success(first)"),
    programs("Set.contains is inverted", COLLECTIONS,
             "success(boolean_value(!map::get(tree, arguments[1]).is_null()))",
             "success(boolean_value(map::get(tree, arguments[1]).is_null()))"),
    programs("Set.reduce passes each item's value too", COLLECTIONS,
             "Some(step) => reduce_in_order(call, step, arguments[1], tree, false),",
             "Some(step) => reduce_in_order(call, step, arguments[1], tree, true),"),
    programs("a reduce keeps going after a failure", COLLECTIONS,
             "            if (*control).kind != ControlKind::SUCCESS {\n                break;\n            }\n",
             ""),
    programs("a reduce starts from its first entry's key", COLLECTIONS,
             "        let mut control = success(initial);",
             "        let mut control = success(if entries.is_empty() { initial } else { (*entries[0]).key });"),
    programs("Option values take the wrong constructor", STANDARD,
             "            return success(standard_value(StandardConstructor::OPTION_NONE, &[]));",
             "            return success(standard_value(StandardConstructor::OPTION_SOME, &[]));"),
    # The field id one further on survived: in the test program it names
    # another `value` field.
    dict(name="standard values name their fields after the variant", file=STANDARD, targets=["language"], filter="a_map_lookup_s_option_names_its_field",
         edits=[(
             "string(*ids.add(shape.fields_offset as usize + field))",
             "string(shape.variant_name + 0 * *ids.add(shape.fields_offset as usize + field))")]),
]
