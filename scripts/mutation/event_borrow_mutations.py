"""Mutants of a Wait event's link to its receiver through calls, breaks and
pattern bindings (CHI-241).

Run: scripts/mutation/mutate.py scripts/mutation/event_borrow_mutations.py
"""

CRATE = "compiler/chimi"

CAPABILITY = "src/capability.rs"
TYPING = "src/typing.rs"
FILTER = "an_event_"


def mutant(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["channel_variants"], filter=FILTER)


MUTATIONS = [
    mutant("a call's event borrows nothing", CAPABILITY, [(
        "                if standard.is_none()\n                    && result_type.is_some_and(",
        "                if false\n                    && result_type.is_some_and(",
    )]),
    mutant("a call's event ignores the argument's receiver", CAPABILITY, [(
        "                        result.borrows.extend(value.carries.iter().copied());\n                        result.borrows.extend(value.borrows.iter().copied());",
        "                        result.borrows.extend(value.borrows.iter().copied());",
    )]),
    mutant("a call's event ignores a receiver read from a field", CAPABILITY, [(
        "                        result\n                            .borrows\n                            .extend(self.field_root_binding(module_id, argument, environment));\n                    }\n                }",
        "                    }\n                }",
    )]),
    mutant("break keeps only carries", CAPABILITY, [(
        "                        breaks.borrows.append(&mut value.borrows);",
        "",
    )]),
    mutant("a loop drops its breaks' borrows", CAPABILITY, [(
        "                loop_value.borrows = breaks.borrows;",
        "",
    )]),
    mutant("a pattern binding borrows nothing", CAPABILITY, [(
        "                value.borrows = subject_borrows.clone();",
        "",
    )]),
    mutant("every pattern binding borrows", CAPABILITY, [(
        "            if self.typed.type_may_hold_select_event(*type_id) {\n                value.borrows",
        "            if true {\n                value.borrows",
    )]),
    mutant("no type holds an event", TYPING, [(
        "                        == Some(StandardSymbol::Type(StandardType::SelectEvent))",
        "                        == Some(StandardSymbol::Type(StandardType::Task))",
    )]),
    mutant("a closure's event borrows nothing", CAPABILITY, [(
        "                    result.borrows.extend(callee_value.borrows.iter().copied());",
        "",
    )]),
    mutant("a field read drops the event's borrows", CAPABILITY, [(
        "                if holds_event {\n                    value.borrows = receiver.borrows.clone();",
        "                if false {\n                    value.borrows = receiver.borrows.clone();",
    )]),
]
