"""Mutants of the needs row lacks rule (CHI-297): a row parameter lacks the
names its signature writes beside it in a parameter's type, a `using` may
supply a name to a row only if the row lacks it (T4225), a call may not bind
a row to a name its callee takes off it (T4226), and a generic member's
provider may not lack more than the member (T4213).

Run: scripts/mutation/mutate.py scripts/mutation/row_lacks_mutations.py
"""

CRATE = "compiler/chimi"

TYPING = "src/typing.rs"

FILTER = "row"


def mutant(name, edits, filter=FILTER):
    return dict(name=name, file=TYPING, edits=edits, targets=["language"], filter=filter)


MUTATIONS = [
    mutant("using never checks the row", [(
        "            if !supplied_names.contains(&need.lexical_name) {\n                continue;\n            }\n            let Some(parameter) = evaluation",
        "            if true {\n                continue;\n            }\n            let Some(parameter) = evaluation",
    )]),
    mutant("calls never check the lacks", [(
        "            self.check_row_lacks(module_id, range, callee_symbol, &parameters, substitution);\n",
        "",
    )]),
    mutant("a generic value's instantiation never checks the lacks", [(
        "        if self.member_conformance_depth == 0 {\n            self.check_row_lacks(",
        "        if false {\n            self.check_row_lacks(",
    )]),
    mutant("a returned function takes nothing off the row", [(
        "        self.collect_row_peels(contract.success, parameter, &mut names, &mut seen);\n        let own",
        "        let own",
    )]),
    mutant("a forwarded name is still lacked", [(
        "            names.retain(|name| !own.entries.iter().any(|need| &need.lexical_name == name));\n",
        "",
    )]),
    mutant("a generic member's row lacks nothing", [(
        "            _ => self.fields.values().flatten().find_map(|field| {",
        "            _ => self.fields.values().flatten().filter(|_| false).find_map(|field| {",
    )]),
    mutant("a provider may lack more than its member", [(
        "        takes_off_more.then_some(",
        "        (takes_off_more && false).then_some(",
    )]),
    mutant("a bound row parameter never clashes", [(
        "                        .find(|other| !self.row_lacked_names(**other).contains(&name))",
        "                        .find(|_| false)",
    )]),
    mutant("peels are read from the parameter's own row only", [(
        "                for input in &contract.inputs {\n                    self.collect_row_peels(*input, parameter, names, seen);\n                }\n                self.collect_row_peels(contract.success, parameter, names, seen);",
        "                let _ = seen;",
    )]),
]
