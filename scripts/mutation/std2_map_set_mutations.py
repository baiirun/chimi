"""Mutants of the std/map and std/set port (77f5f4e..848dc2d): the C twin of
the native core in runtime/native/runtime.c, the Rust model in src/execution.rs
and src/execution/map_operations.rs, the Chimi wrappers in std/map.chimi, and
the key requirements in src/operations.rs. C and Rust sides are mutated
separately because std_modules compares both backends against the same
expected files.

Run: CARGO_BUILD_JOBS=2 scripts/mutation/mutate.py --scratch /tmp/claude-0/mscratch-map \\
         --revision 848dc2d scripts/mutation/std2_map_set_mutations.py
Each mutant must turn a std-tests/map or std-tests/set program (either
backend) or the std_modules key-requirement test red. Filters other than
"standard_programs" select no program test in std_modules.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
EXECUTION = "src/execution.rs"
OPERATIONS = "src/execution/map_operations.rs"
DEBUG = "src/execution/debug.rs"
REGISTRY = "src/operations.rs"
MAP = "std/map.chimi"


def programs(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_programs")


def requirements(name, file, edits):
    # The typing test lives in std_modules beside the programs.
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="map_and_set_state_their_key_requirements")


MUTATIONS = [
    programs("C map remove skips rebalancing on the left", RUNTIME, [(
        "return left == node->left ? node : map_balance(map_node(node->key, node->value, left, node->right));",
        "return left == node->left ? node : map_node(node->key, node->value, left, node->right);")]),
    programs("Rust map remove skips rebalancing", EXECUTION, [(
        "        Some(Some(map_balance(map_node(\n            node.key.clone(),\n            node.value.clone(),\n            left,\n            right,\n        ))))",
        "        Some(Some(map_node(\n            node.key.clone(),\n            node.value.clone(),\n            left,\n            right,\n        )))")]),
    programs("C map remove of a node without a left child drops its right subtree", RUNTIME, [(
        "if (!node->left) return node->right;\n  if (!node->right) return node->left;\n  MapNode *least = NULL;",
        "if (!node->left) return node->left;\n  if (!node->right) return node->left;\n  MapNode *least = NULL;")]),
    programs("Rust map remove with two children takes the least of the left subtree's sibling wrongly", EXECUTION, [(
        "(None, right) => right.clone(),",
        "(None, _) => None,")]),
    programs("C Set.add replaces the held equal item instead of keeping it", RUNTIME, [(
        "return success(map_get_node(set, arguments[1]) ? arguments[0]\n                                                      : set_value(map_put_node(set, arguments[1], void_value())));",
        "return success(set_value(map_put_node(set, arguments[1], void_value())));")]),
    programs("C map size is one short for a leaf", RUNTIME, [(
        "return node ? 1 + map_size(node->left) + map_size(node->right) : 0;",
        "return node ? (node->left || node->right ? 1 : 0) + map_size(node->left) + map_size(node->right) : 0;")]),
    programs("C reduce_entries visits entries in descending order", RUNTIME, [(
        "/* The native core of std/map; execution/map_operations.rs is the interpreter's. */",
        "static MapEntries reversed_entries(const MapNode *root) {\n"
        "  MapEntries e = map_entries(root);\n"
        "  for (size_t i = 0; i < e.length / 2; ++i) {\n"
        "    const MapNode *t = e.nodes[i]; e.nodes[i] = e.nodes[e.length - 1 - i]; e.nodes[e.length - 1 - i] = t;\n"
        "  }\n"
        "  return e;\n"
        "}\n"
        "/* The native core of std/map; execution/map_operations.rs is the interpreter's. */"),
        ("arguments[1], map_entries(map), true,",
         "arguments[1], reversed_entries(map), true,")]),
    programs("Map.merge lets the base win over overrides", MAP, [(
        "reduce_entries(overrides, map, put)",
        "reduce_entries(map, overrides, put)")]),
    programs("Map.from keeps the first pair for a repeated key", MAP, [(
        "(key, value) -> put(map, key, value)",
        "(key, value) -> case has_key(map, key) {\n        True -> map\n        False -> put(map, key, value)\n      }")]),
    programs("Rust Set renders in Map's form", DEBUG, [(
        'self.composite("Set ", "{", "}", true, parts);',
        'self.composite("Map ", "{", "}", true, parts);')]),
    requirements("Map.reduce no longer requires an ordered key", REGISTRY, [(
        '    requires("std/map.reduce", "K", ValueProperty::Ordered),\n', "")]),
    requirements("Set items are no longer required to be equatable (no T4300)", REGISTRY, [
        ('    requires("std/set.empty", "T", ValueProperty::Equatable),\n', ""),
        ('    requires("std/set.contains", "T", ValueProperty::Equatable),\n', ""),
        ('    requires("std/set.add", "T", ValueProperty::Equatable),\n', ""),
        ('    requires("std/set.remove", "T", ValueProperty::Equatable),\n', "")]),
]
