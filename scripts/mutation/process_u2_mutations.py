"""Mutants of supervised-process stage U2 (ts-6ccbc9): the process capability rules
(T4260, Process.send as a transfer boundary, consuming removal, inbox escape), the
Core verifier, and process-name identity in Core.

Run: scripts/mutation/mutate.py scripts/mutation/process_u2_mutations.py
Each mutant must turn the lab (tests/process_lab.rs), a U2 language test, or the
verifier unit test red. Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.
"""

CRATE = "compiler/chimi"

C = "src/capability.rs"
P = "src/typing/process.rs"
V = "src/executable/verify.rs"
E = "src/executable.rs"
L = "src/process_lab/static_kind.rs"


def lab(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["process_lab"], filter="")


def language(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


def unit(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=filter)


MUTATIONS = [
    # T4260.
    lab("T4260 capture check dropped", C, [(
        "        } else if body.properties.shareable == PropertyDerivation::Never\n",
        "        } else if false\n")]),
    language("T4260 provider attached to the body not checked", C, [(
        "        if let Some(provider) = &body.nonshareable_provider {",
        "        if let Some(provider) = None::<&String> {")], "a_spec_provider"),
    lab("T4260 provider bound by an outer using not checked", C, [(
        "                    if inner_spec_needs.remove(&provider.name.text)\n"
        "                        && provided.properties.shareable == PropertyDerivation::Never",
        "                    if inner_spec_needs.remove(&provider.name.text)\n"
        "                        && false")]),
    language("T4260 flags every non-shareable provider, not only a spec's", C, [(
        "                    if inner_spec_needs.remove(&provider.name.text)\n",
        "                    if (inner_spec_needs.remove(&provider.name.text) || true)\n")],
        "a_spec_provider"),
    lab("spec needs not followed through function calls (no fixed point)", C, [(
        "        if checker.next_summaries == summaries {",
        "        if true {")]),
    # Process.send.
    lab("Process.send is not a transfer boundary", C, [(
        "                    Some(StandardFunction::ProcessSend) => {",
        "                    Some(StandardFunction::ProcessSend) if false => {")]),
    lab("send moves only a payload that is itself a binding", C, [(
        "                if constructs {",
        "                if false {")]),
    language("send payload list literals not followed", C, [(
        "            ModuleExpression::List { elements, .. } | ModuleExpression::Tuple { elements, .. } => {\n"
        "                for element in elements {\n"
        "                    self.payload_bindings(",
        "            ModuleExpression::Tuple { elements, .. } => {\n"
        "                for element in elements {\n"
        "                    self.payload_bindings(")], "a_send_moves_bindings"),
    language("send payload tuple literals not followed", C, [(
        "            ModuleExpression::List { elements, .. } | ModuleExpression::Tuple { elements, .. } => {\n"
        "                for element in elements {\n"
        "                    self.payload_bindings(",
        "            ModuleExpression::List { elements, .. } => {\n"
        "                for element in elements {\n"
        "                    self.payload_bindings(")], "a_send_moves_bindings"),
    language("send payload record literals not followed", C, [(
        "                for field in fields {\n"
        "                    self.payload_bindings(module_id, &field.value, bindings);",
        "                for field in fields {\n"
        "                    let _ = field;")], "a_send_moves_bindings"),
    language("send payload parentheses not followed", C, [(
        "            ModuleExpression::Group { expression, .. } => {\n"
        "                self.payload_bindings(module_id, expression, bindings);",
        "            ModuleExpression::Group { expression, .. } => {\n"
        "                let _ = expression;")], "a_send_moves_bindings"),
    language("send payload constructors followed one level only", C, [(
        "                    for argument in arguments {\n"
        "                        self.payload_bindings(module_id, &argument.value, bindings);",
        "                    for argument in arguments {\n"
        "                        if let ModuleExpression::Name { semantic_id, .. } = &argument.value\n"
        "                            && let Some(symbol) = self.resolved_value_symbol(module_id, *semantic_id)\n"
        "                        {\n"
        "                            bindings.push((symbol, argument.value.range()));\n"
        "                        }")], "a_send_moves_bindings"),
    language("send payload follows any function call", C, [(
        "                if constructs {",
        "                if constructs || true {")], "a_send_moves_bindings"),
    language("send moves shareable values too", C, [(
        "            if environment.get(&symbol).is_some_and(|binding| {\n"
        "                binding.value.properties.capability == CapabilityClass::Exclusive\n"
        "            }) {",
        "            if environment.contains_key(&symbol) {")], "a_send_moves"),
    # Removal.
    lab("remove_process does not consume", C, [(
        "                        function @ (StandardFunction::SupervisorRemoveProcess\n"
        "                        | StandardFunction::SupervisorRemoveSupervisor),",
        "                        function @ StandardFunction::SupervisorRemoveSupervisor,")]),
    language("remove_supervisor does not consume", C, [(
        "                        function @ (StandardFunction::SupervisorRemoveProcess\n"
        "                        | StandardFunction::SupervisorRemoveSupervisor),",
        "                        function @ StandardFunction::SupervisorRemoveProcess,")],
        "removing_a_supervisor"),
    # Inbox escape.
    lab("T4240 for a body failure type dropped", P, [(
        "        if self.typed.value_properties(failure).transferable != PropertyDerivation::Never {",
        "        if true {")]),
    # Core verifier.
    lab("verifier CO02 check dropped", V, [(
        "                    if !named {",
        "                    if false {")]),
    lab("verifier CO04 check dropped", V, [(
        "        if self.is_inbox(node.type_id) && !self.reads_an_inbox(node) {",
        "        if false {")]),
    lab("verifier accepts an inbox read from any symbol", V, [(
        "                self.core.symbol(*symbol).kind == CoreSymbolKind::Local\n",
        "                true\n")]),
    lab("verifier accepts an inbox made from a literal", V, [(
        "            CoreOperation::Constant(_)\n            | CoreOperation::DependencyMethod { .. }",
        "            CoreOperation::Constant(_) => true,\n            CoreOperation::DependencyMethod { .. }")]),
    unit("verifier spec-body rule dropped", V, [(
        "                        && let Some(violation) = not_shareable(body, *type_id)",
        "                        && let Some(violation) = None::<CoreViolation>")],
        "the_verifier_rejects"),
    # Core identity.
    lab("process names keyed by text in Core (CO01)", E, [(
        "    fn reference(&self, symbol: SymbolId) -> CoreOperation {\n",
        "    fn reference(&self, symbol: SymbolId) -> CoreOperation {\n"
        "        let resolved = &self.typed.resolved;\n"
        "        let symbol = if resolved.symbol(symbol).kind == ProjectSymbolKind::ProcessName {\n"
        "            resolved.symbols.iter().find(|other| other.kind == ProjectSymbolKind::ProcessName\n"
        "                && other.name == resolved.symbol(symbol).name).map_or(symbol, |other| other.id)\n"
        "        } else {\n"
        "            symbol\n"
        "        };\n")]),
    lab("a user function named like a process operation is an intrinsic (CO03)", E, [(
        "                ProjectSymbolKind::Function => CoreSymbolKind::Function,",
        "                ProjectSymbolKind::Function if symbol.name == \"send\" => CoreSymbolKind::Intrinsic,\n"
        "                ProjectSymbolKind::Function => CoreSymbolKind::Function,")]),
    # The static runner.
    lab("static runner reads project contracts as standard ones", L, [(
        "                Some((module, function)) if module.contains('/') => {",
        "                Some((module, function)) if false => {")]),
]
