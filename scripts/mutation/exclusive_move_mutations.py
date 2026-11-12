"""Mutants of the exclusive-move rule (ts-184972): a value built from an exclusive
binding carries it, and binding or transferring the value moves what it carries.

Run: scripts/mutation/mutate.py scripts/mutation/exclusive_move_mutations.py
Each mutant must turn a capability test in tests/language.rs red. Results:
scripts/mutation/exclusive_move_results.txt.
"""

CRATE = "compiler/chimi"

C = "src/capability.rs"
T = "src/typing.rs"


def language(name, edits, filter, file=C):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


MOVES = "an_exclusive_binding_moves"
READS = "reading_or_owning"

MUTATIONS = [
    # What a value carries.
    language("a name of an exclusive binding carries nothing", [(
        "                        value.carries.insert(symbol);",
        "                        let _ = symbol;")], "a_send_moves"),
    language("a name of a shareable binding carries it too", [(
        "                    if !binding.requirement\n"
        "                        && value.properties.capability == CapabilityClass::Exclusive\n",
        "                    if !binding.requirement\n")], "a_send_moves_the_exclusive"),
    language("combining values drops what the later one carries", [(
        "        self.carries.append(&mut other.carries);",
        "        other.carries.clear();")], "a_send_moves_bindings"),
    language("a case carries only its first arm's value", [(
        "                values\n"
        "                    .into_iter()\n"
        "                    .fold(FlowValue::shareable(), FlowValue::combine)",
        "                values\n"
        "                    .into_iter()\n"
        "                    .next()\n"
        "                    .unwrap_or_else(FlowValue::shareable)")], MOVES),
    language("a case carries only its last arm's value", [(
        "                values\n"
        "                    .into_iter()\n"
        "                    .fold(FlowValue::shareable(), FlowValue::combine)",
        "                values\n"
        "                    .into_iter()\n"
        "                    .last()\n"
        "                    .unwrap_or_else(FlowValue::shareable)")], MOVES),
    language("a record update carries only its base", [(
        "                    .fold(base, FlowValue::combine)",
        "                    .fold(base, |base, field| {\n"
        "                        let _ = field;\n"
        "                        base\n"
        "                    })")], MOVES),
    language("a call's result never carries its arguments", [(
        "                        if embeds {\n                            result.carries.extend(value.carries);",
        "                        if false && embeds {\n                            result.carries.extend(value.carries);")],
        MOVES),
    language("a call's result does not carry its callee", [(
        "                    result.carries = callee_value.carries;",
        "                    result.carries = BTreeSet::new();\n"
        "                    let _ = callee_value;")], MOVES),
    language("a call with a shareable result carries its arguments", [(
        "                if result.properties.shareable != PropertyDerivation::Always {\n"
        "                    result.carries = callee_value.carries;",
        "                if true {\n"
        "                    result.carries = callee_value.carries;")], READS),
    language("any exclusive argument flows into a non-shareable result", [(
        "                                self.typed.type_may_hold(result_type, argument_type)",
        "                                true")], READS),
    language("no argument type is held by a result type", [(
        "        self.type_may_hold_with(container, part, &HashMap::new(), 0)",
        "        false")], MOVES, file=T),
    language("held types ignore struct and variant fields", [(
        "                fields.iter().any(|field| holds(field.type_id, &inner))\n"
        "                    || arguments",
        "                arguments")], MOVES, file=T),
    language("a type parameter of the container may hold anything", [(
        "            return self.type_may_hold_with(*bound, part, generics, depth + 1);",
        "            return true;")], READS, file=T),
    language("type identity ignores type arguments", [(
        "                    && arguments.len() == other_arguments.len()\n"
        "                    && arguments\n"
        "                        .iter()\n"
        "                        .zip(other_arguments)\n"
        "                        .all(|(a, b)| same(a, b))",
        "                    && arguments.len() == other_arguments.len()")], READS, file=T),
    language("a break value carries nothing out of its loop", [(
        "                        breaks.extend(carries);",
        "                        let _ = (breaks, carries);")], MOVES),
    language("a field read out of a binding carries the binding", [(
        "                let carries = if value.properties.shareable == PropertyDerivation::Always\n"
        "                    || reads_a_binding(receiver_expression)\n",
        "                let carries = if value.properties.shareable == PropertyDerivation::Always\n")],
        READS),
    language("a field read out of a built value carries nothing", [(
        "                    receiver.carries.clone()",
        "                    BTreeSet::new()")], MOVES),
    # Where carried bindings move.
    language("a let binding moves nothing", [(
        "                TransferBoundary::IntoBinding(owner),\n                environment,\n            );",
        "                TransferBoundary::IntoBinding(owner),\n                &mut environment.clone(),\n            );")],
        MOVES),
    language("a loop state moves nothing", [(
        "            self.transfer_carried(&initial_value, initial.range(), boundary, environment);",
        "            let _ = boundary;")], MOVES),
    language("a destructured or matched built value moves nothing", [(
        "                    if !reads_a_binding(subject) {",
        "                    if false {")], MOVES),
    language("matching a binding moves it", [(
        "                    if !reads_a_binding(subject) {",
        "                    if true {")], READS),
    language("a using provider transfers nothing", [(
        "                    self.transfer_carried(\n"
        "                        &provided,",
        "                    let _ = &provided;\n"
        "                    self.transfer_carried(\n"
        "                        &FlowValue::invalid(),")], MOVES),
    language("a send transfers nothing it carries", [(
        "        self.transfer_carried(payload, argument.range(), boundary, environment);",
        "        let _ = boundary;")], "a_send_moves"),
]
