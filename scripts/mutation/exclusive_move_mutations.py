"""Mutants of the exclusive-move rule (ts-184972): a value built from an exclusive
place carries it, and binding or transferring the value moves what it carries.
Parts of a binding move on their own (ts-05c0da), a call moves what its callee
transfers (ts-a2fc84), and `Channel.Receive<T>` is what `T` is (ts-bb95dc).

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
PARTS = "an_exclusive_part_has_one_owner"
CALLS = "a_call_of_a_function_that_transfers"
RECEIVED = "a_received_value"
RECURSION = "recursion_over_parts"
LOOP = "building_a_value_from_an_outer"

MUTATIONS = [
    # What a value carries.
    language("a read of an exclusive place carries nothing", [(
        "            value.carries.insert(place);",
        "            let _ = place;")], "a_send_moves"),
    language("a read of a shareable place carries it too", [(
        "        if let Some(place) = place\n"
        "            && owned(&value)\n",
        "        if let Some(place) = place\n")], "a_send_moves_the_exclusive"),
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
        "                let mut value = self.evaluate(module_id, base, environment);\n"
        "                for field in fields {\n"
        "                    let part = self.evaluate(module_id, &field.value, environment);\n"
        "                    value = self.join_part(module_id, value, field.value.range(), part);",
        "                let mut value = self.evaluate(module_id, base, environment);\n"
        "                for field in fields {\n"
        "                    let part = self.evaluate(module_id, &field.value, environment);\n"
        "                    let _ = part;")], MOVES),
    language("a call's result never carries its arguments", [(
        "                        if embeds {\n                            self.hold_part(",
        "                        if false && embeds {\n                            self.hold_part(")],
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
    language("a field read out of a binding is the whole binding", [(
        "                self.expression_place(module_id, receiver, environment)?\n"
        "                    .child(PathSegment::Field(member.text.clone())),",
        "                self.expression_place(module_id, receiver, environment)?,")],
        READS),
    language("a field read out of a built value carries nothing", [(
        "            receiver.carries.clone()",
        "            BTreeSet::new()")], MOVES),
    # Where carried bindings move.
    language("a let binding moves nothing", [(
        "            self.transfer_place(place, at, TransferBoundary::IntoBinding(owner), environment);",
        "            let into = TransferBoundary::IntoBinding(owner);\n"
        "            self.transfer_place(place, at, into, &mut environment.clone());")],
        MOVES),
    language("a loop state moves nothing", [(
        "            self.transfer_carried(&initial_value, initial.range(), boundary, environment);",
        "            let _ = boundary;")], READS),
    language("a destructured or matched built value moves nothing", [(
        "                            TransferBoundary::IntoPattern,\n"
        "                            environment,",
        "                            TransferBoundary::IntoPattern,\n"
        "                            &mut environment.clone(),")], MOVES),
    language("matching a binding moves it", [(
        "                        self.record(module_id, subject, value);\n"
        "                        subject_carries.push(BTreeSet::new());",
        "                        let at = subject.range();\n"
        "                        let into = TransferBoundary::IntoPattern;\n"
        "                        self.transfer_carried(&value, at, into, environment);\n"
        "                        self.record(module_id, subject, value);\n"
        "                        subject_carries.push(BTreeSet::new());")], READS),
    language("a using provider transfers nothing", [(
        "                    self.transfer_carried(\n"
        "                        &provided,",
        "                    let _ = &provided;\n"
        "                    self.transfer_carried(\n"
        "                        &FlowValue::invalid(),")], MOVES),
    language("a send transfers nothing it carries", [(
        "        self.transfer_carried(payload, argument.range(), boundary, environment);",
        "        let _ = boundary;")], "a_send_moves"),
    language("a closure capture moves nothing", [(
        "                    TransferBoundary::ClosureCapture,\n"
        "                    environment,\n"
        "                );\n"
        "            }\n"
        "            closure = closure.combine(captured);",
        "                    TransferBoundary::ClosureCapture,\n"
        "                    &mut environment.clone(),\n"
        "                );\n"
        "            }\n"
        "            closure = closure.combine(captured);")], RECEIVED),
    # Parts of a binding (ts-05c0da).
    language("a field read of a binding is not checked", [(
        "                        *range,\n"
        "                        ReadCheck::Whole,",
        "                        *range,\n"
        "                        ReadCheck::Paths(&BTreeSet::new()),")], PARTS),
    language("a moved part leaves its whole binding usable", [(
        "    left.iter()\n"
        "        .zip(right)\n"
        "        .all(|(left, right)| left.may_reach_the_same_part(right))",
        "    left == right")], PARTS),
    language("sibling fields reach the same part", [(
        "            (Self::Field(a), Self::Field(b)) => a == b,",
        "            (Self::Field(_), Self::Field(_)) => true,")], READS),
    language("a pattern binding over a binding is a new owner", [(
        "                    part_of: place.clone(),",
        "                    part_of: None,")], PARTS),
    language("a value may hold one place twice", [(
        "        self.check_held_once(module_id, &whole.carries, at, &part.carries);",
        "        let _ = at;")], PARTS),
    language("the loop rule sees only whole-binding moves", [(
        "                    .filter(|(path, _)| !before.moved.iter().any(|(moved, _)| moved == path))",
        "                    .filter(|(path, _)| {\n"
        "                        path.is_empty() && !before.moved.iter().any(|(moved, _)| moved == path)\n"
        "                    })")], PARTS),
    language("a closure captures whole bindings", [(
        "            captures.entry(place).or_insert(expression);",
        "            captures.entry(Place::whole(place.root)).or_insert(expression);")], READS),
    # Calls of functions that transfer a parameter (ts-a2fc84).
    language("a call moves nothing its callee transfers", [(
        "        let mut taken = Vec::<Place>::new();",
        "        let transfers = &ParameterPaths::new();\n"
        "        let mut taken = Vec::<Place>::new();")], CALLS),
    language("transfer summaries stop after one pass", [(
        "        if checker.next_summaries == summaries {",
        "        if checker.next_summaries.spec_needs == summaries.spec_needs {")], CALLS),
    language("a summary transfers every parameter", [(
        "            (!paths.is_empty()).then_some((index, paths))",
        "            let _ = paths;\n"
        "            Some((index, BTreeSet::from([Vec::new()])))")], READS),
    language("a closure's parameter transfers are dropped", [(
        "        closure.parameter_transfers = parameter_paths(&parameters, &self.escaped);",
        "        let _ = parameter_paths(&parameters, &self.escaped);")], CALLS),
    language("one call may take a place for two parameters", [(
        "                .find(|place| taken.iter().any(|taken| taken.overlaps(place)))",
        "                .find(|_| false)")], CALLS),
    language("a call moves the whole argument, not the transferred parts", [(
        "                Some(place) => paths.iter().map(|path| place.joined(path)).collect(),",
        "                Some(place) => paths.iter().map(|_| place.clone()).collect(),")], READS),
    # Review fixes: bounded paths, list and tuple parts, escapes, reads, generics.
    language("paths are not cut to a bounded depth", [(
        "        place.path.truncate(MAX_PATH_DEPTH);",
        "        let _ = MAX_PATH_DEPTH;")], RECURSION),
    language("list steps always overlap", [(
        "                            (Some((a, b)), Some((c, d))) => a < d && c < b,",
        "                            (Some(_), Some(_)) => true,")], RECURSION),
    language("list steps never overlap", [(
        "                            (Some((a, b)), Some((c, d))) => a < d && c < b,",
        "                            (Some(_), Some(_)) => false,")], PARTS),
    language("tuple positions overlap", [(
        "            (Self::Position(a), Self::Position(b)) => a == b,",
        "            (Self::Position(_), Self::Position(_)) => true,")], READS),
    language("a move into a local escapes", [(
        "        !matches!(self, Self::IntoBinding(_) | Self::IntoPattern)",
        "        let _ = self;\n        true")], READS),
    language("an escape does not follow what a binding took", [(
        "        for taken in &took {",
        "        for taken in took.iter().take(0) {")], LOOP),
    language("a call checks the whole argument", [(
        "                    .filter(|symbol| self.is_project_function(*symbol))",
        "                    .filter(|_| false)")], READS),
    language("read summaries are empty", [(
        "            let reads = parameter_paths(&parameters, &self.read_places);",
        "            let reads = ParameterPaths::new();")], CALLS),
    language("a case subject is checked whole", [(
        "                            self.read_argument(module_id, subject, ReadCheck::Tag, environment);",
        "                            self.read_argument(module_id, subject, ReadCheck::Whole, environment);")],
        READS),
    language("values of a type parameter are not tracked", [(
        "        || (value.holds_type_parameter\n",
        "        || (false\n")], CALLS),
    language("function values are tracked like a type parameter", [(
        "        || (value.holds_type_parameter\n"
        "            && value.properties.shareable",
        "        || (value.properties.shareable")], READS),
    language("a type parameter is not a held type parameter", [(
        "            CanonicalType::Generic(_) => true,",
        "            CanonicalType::Generic(_) => false,")], CALLS, file=T),
    # `Channel.Receive<T>` (ts-bb95dc).
    language("Receive<T> is shareable whatever T is", [(
        "                StandardType::ProcessSendError | StandardType::ChannelReceive,",
        "                StandardType::ProcessSendError,")], RECEIVED, file=T),
    language("Receive<T> is always exclusive", [(
        "                StandardType::ChannelReceiver | StandardType::ChannelCompletion,\n"
        "            )) => {",
        "                StandardType::ChannelReceiver\n"
        "                | StandardType::ChannelCompletion\n"
        "                | StandardType::ChannelReceive,\n"
        "            )) => {")], RECEIVED, file=T),
]
