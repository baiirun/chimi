"""Mutants of module constant decoding in Rust (CHI-305 step 5, phase 8,
eighth PR, runtime/chimi-runtime/src/constants.rs). The language tests run
each program on both backends, so a constant the native runtime decodes
wrongly shows up as a difference from the interpreter.

Run: scripts/mutation/mutate.py scripts/mutation/constants_mutations.py
Results: scripts/mutation/constants_results.txt.
"""

CRATE = "compiler/chimi"

CONSTANTS = "../../runtime/chimi-runtime/src/constants.rs"


def mutant(name, old, new, targets=("language", "cli")):
    return dict(name=name, file=CONSTANTS, edits=[(old, new)], targets=list(targets), filter="")


MUTATIONS = [
    mutant("a record constant loses its field names",
           "                    name: string(id(2 * index)),",
           "                    name: string(id(2 * index + 1)),"),
    mutant("a decimal constant loses its sign",
           "                    negative: raw.a & 0x100 != 0,",
           "                    negative: false,"),
    mutant("a bytes constant swaps each byte's digits",
           "hex_value(hex[2 * index]) << 4 | hex_value(hex[2 * index + 1])",
           "hex_value(hex[2 * index + 1]) << 4 | hex_value(hex[2 * index])"),
    mutant("a constant function binds no requirements",
           "                bind_requirements(constants, (*callable).environment, ids, raw.integer);\n",
           ""),
    mutant("a constant closure binds no requirements",
           "                bind_requirements(constants, environment, ids.add(2 * raw.length as usize), raw.integer);\n",
           ""),
    mutant("a set constant is a map",
           "                return pointer_value(ValueKind::SET, node.cast());",
           "                return pointer_value(ValueKind::MAP, node.cast());"),
    mutant("a float constant reads its bits as an integer",
           "f64::from_bits(raw.integer as u64)",
           "raw.integer as f64"),
    mutant("a named address constant names the next process",
           "(*value).payload.address.target = u64::from(raw.a) + 1;",
           "(*value).payload.address.target = u64::from(raw.a) + 2;"),
    mutant("a list constant drops its last item",
           "                return list::list_value(list::from_values(items, raw.length), 0, raw.length);",
           "                return list::list_value(list::from_values(items, raw.length), 0, raw.length.saturating_sub(1));"),
]
