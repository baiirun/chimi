"""Mutants of reading a host-served need as a value (CHI-295): Core lowering
builds a record of the interface's methods, each a dependency method.

Run: scripts/mutation/mutate.py scripts/mutation/host_need_value_mutations.py
"""

CRATE = "compiler/chimi"

EXECUTABLE = "src/executable.rs"
OPERATIONS = "src/operations.rs"

FILTER = "a_host_served_need"


def mutant(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=FILTER)


MUTATIONS = [
    mutant("the need is read as a plain reference", EXECUTABLE, [(
        "            _ => self\n                .host_capability_value(module_id, expression, symbol)\n                .unwrap_or_else(|| self.reference(symbol)),",
        "            _ => self.reference(symbol),",
    )]),
    mutant("console is not host-served", OPERATIONS, [(
        "        StandardType::Console | StandardType::FileSystem | StandardType::Random",
        "        StandardType::FileSystem | StandardType::Random",
    )]),
    mutant("random is not host-served", OPERATIONS, [(
        "        StandardType::Console | StandardType::FileSystem | StandardType::Random",
        "        StandardType::Console | StandardType::FileSystem",
    )]),
    mutant("the record keeps only the first method", EXECUTABLE, [(
        "        let methods = self.typed.fields.get(&interface)?.clone();",
        "        let methods = self.typed.fields.get(&interface)?.iter().take(1).cloned().collect::<Vec<_>>();",
    )]),
    mutant("the record keeps only the last method", EXECUTABLE, [(
        "        let methods = self.typed.fields.get(&interface)?.clone();",
        "        let methods = self.typed.fields.get(&interface)?.iter().skip(1).cloned().collect::<Vec<_>>();",
    )]),
]
