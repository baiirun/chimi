"""Mutants of the Map tree and structural order in Rust (CHI-305 step 5,
phase 4, third PR; runtime/chimi-runtime/src/heap/map.rs, compare.rs and
json_number.rs). They are caught by the std map, set and json programs
(std_modules), the language suite, and the white-box fixtures native-map-runtime.c and
native-structural-compare.c (tests/cli.rs). Older mutants of the same code,
retargeted from runtime.c, are in std2_map_set_mutations.py,
std2_json_mutations.py, compare_stack_mutations.py and
lane_fixes_mutations.py.

Run: scripts/mutation/mutate.py scripts/mutation/map_compare_mutations.py
Results: scripts/mutation/map_compare_results.txt.
"""

CRATE = "compiler/chimi"
HEAP = "../../runtime/chimi-runtime/src/heap/"
MAP = HEAP + "map.rs"
COMPARE = HEAP + "compare.rs"
NUMBER = HEAP + "json_number.rs"


def programs(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["std_modules"], filter="standard_")


def language(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["language"], filter="")


def balanced(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["cli"],
                filter="native_map_removal_keeps_the_tree_balanced")


def structural(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["cli"],
                filter="native_structural_comparison_handles_deep_nesting_and_exact_json_numbers")


MUTATIONS = [
    balanced("a left-heavy node never rotates twice", MAP,
             "            let node = if height((*left).right) > height((*left).left) {",
             "            let node = if false {"),
    balanced("a right-heavy node never rotates twice", MAP,
             "            let node = if height((*right).left) > height((*right).right) {",
             "            let node = if false {"),
    balanced("rebalancing waits for a difference of three", MAP,
             "        if difference > 1 {",
             "        if difference > 2 {"),
    programs("a left rotation drops the inner subtree", MAP,
             "        let left = self::node((*node).key, (*node).value, (*node).left, (*right).left);",
             "        let left = self::node((*node).key, (*node).value, (*node).left, ptr::null_mut());"),
    programs("removing the least node keeps it", MAP,
             "            *least = node;\n            return (*node).right;",
             "            *least = node;\n            return node;"),
    programs("a lookup turns the wrong way", MAP,
             "            node = if order < 0 { (*node).left } else { (*node).right };",
             "            node = if order < 0 { (*node).right } else { (*node).left };"),
    programs("entries skip right subtrees", MAP,
             "            nodes.push(node);\n            node = (*node).right;",
             "            nodes.push(node);\n            node = ptr::null();"),
    programs("maps compare their keys only", COMPARE,
             "                        stack.push((*left_node).value, (*right_node).value, 0);\n",
             ""),
    language("lists ignore their lengths", COMPARE,
             "                    stack.decide(order_of(left_length, right_length));",
             "                    stack.decide(0);"),
    language("values of different kinds compare equal", COMPARE,
             "                order = order_of((*left).kind.0, (*right).kind.0);",
             "                order = 0;"),
    structural("an exponent sum drops its carries", NUMBER,
               "            carry = i32::from(if subtract { digit < 0 } else { digit > 9 });",
               "            carry = 0;"),
    structural("negative numbers keep the order of their magnitudes", NUMBER,
               "        if left.sign < 0 {\n            order = order.reverse();",
               "        if false {\n            order = order.reverse();"),
    structural("a longer mantissa ties with its prefix", NUMBER,
               "then_with(|| left.digits.cmp(&right.digits))",
               "then_with(|| { let n = left.digits.len().min(right.digits.len()); left.digits[..n].cmp(&right.digits[..n]) })"),
    structural("the decimal point does not move the exponent", NUMBER,
               "digits, exponent: written.sum(&offset) }",
               "digits, exponent: written }"),
    programs("1e21 renders positionally", NUMBER,
             "number.exponent.order(&Exponent::of(21)).is_le()",
             "number.exponent.order(&Exponent::of(22)).is_le()"),
]
