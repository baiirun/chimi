"""Mutants of supervised-process stage U1 (ts-6ccbc9): the static rules T4261-T4264,
the std/process and std/supervisor contracts and value properties, and the
interpreter's data constructors.

Run: scripts/mutation/mutate.py scripts/mutation/process_u1_mutations.py
Each mutant must turn the lab's static fixtures (tests/process_lab.rs) or the U1
language tests red. Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.
"""

CRATE = "compiler/chimi"

R = "src/typing/process.rs"
I = "src/execution/processes.rs"
K = "src/execution/constants.rs"
Y = "src/typing.rs"
L = "src/process_lab/static_kind.rs"
N = "src/backend/native.rs"


def lab(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["process_lab"], filter="")


def cli(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter="native_build_refuses")


def language(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter="")


MUTATIONS = [
    # T4261.
    lab("T4261 dropped", R, [(
        "        if self.typed.value_properties(message).transferable != PropertyDerivation::Never {",
        "        if true {")]),
    lab("T4261 only at declarations, not at Process.spec", R, [(
        '                    self.check_transferable(module_id, body.range(), message, "this process body");',
        "                    let _ = (body, message);")]),
    # T4262.
    lab("T4262 dropped", R, [(
        "                    && self.process_name_reference(module_id, name).is_none()",
        "                    && false")]),
    language("T4262 accepts a constant holding a name", R, [(
        "        (self.typed.resolved.symbol(symbol).kind == ProjectSymbolKind::ProcessName)",
        "        matches!(self.typed.resolved.symbol(symbol).kind, ProjectSymbolKind::ProcessName | ProjectSymbolKind::Constant)")]),
    # T4263.
    lab("T4263 mailbox capacity bound off by one", R, [(
        "                    && let Some(value) = integer_literal(capacity)\n                    && value < 1",
        "                    && let Some(value) = integer_literal(capacity)\n                    && value < 0")]),
    lab("T4263 restart window accepts zero", R, [(
        "                    && value <= 0\n",
        "                    && value < 0\n")]),
    lab("T4263 members-over-capacity check dropped", R, [(
        "                        && elements.len() as i64 > capacity",
        "                        && false")]),
    language("T4263 literal test loosened to any folded integer", R, [(
        "fn integer_literal(expression: &ModuleExpression) -> Option<i64> {\n    match strip_groups(expression) {",
        "fn integer_literal(expression: &ModuleExpression) -> Option<i64> {\n    match strip_groups(expression) {\n        ModuleExpression::Binary { left, right, .. } => integer_literal(left).zip(integer_literal(right)).map(|(l, r)| l - r),\n        ModuleExpression::Name { .. } => Some(0),")]),
    language("T4263 literal duration test loosened to any folded duration", R, [(
        "        if !literal {\n            return None;\n        }\n",
        "        let _ = literal;\n")]),
    # T4264.
    lab("T4264 dropped", R, [(
        "            if let Some(first) = seen.get(&name) {",
        "            if let Some(first) = None::<&usize> {")]),
    lab("T4264 misses the constant follow", R, [(
        "        if !following.insert(symbol) {\n            return None;\n        }",
        "        if following.insert(symbol) {\n            return None;\n        }")]),
    lab("T4264 keys names by text", R, [
        ("        let mut seen = HashMap::<SymbolId, usize>::new();",
         "        let mut seen = HashMap::<String, usize>::new();"),
        ("            if let Some(first) = seen.get(&name) {",
         "            if let Some(first) = seen.get(&self.typed.resolved.symbol(name).name) {"),
        ("                seen.insert(name, index);",
         "                seen.insert(self.typed.resolved.symbol(name).name.clone(), index);"),
    ]),
    # Contracts and value properties.
    lab("Supervisor.start row has every StartError variant", "src/typing/process.rs", [(
        "                failures: name_taken,",
        "                failures: start_failures,")]),
    language("Process.spec drops the body's needs", R, [(
        "                inputs: vec![body, int, duration],\n                success: spec,\n                failures: pure.0,\n                needs,",
        "                inputs: vec![body, int, duration],\n                success: spec,\n                failures: pure.0,\n                needs: pure.1,")]),
    language("Inbox is shareable", Y, [(
        "            Some(StandardSymbol::Type(StandardType::ProcessInbox)) => {\n                return ValueProperties::PARENT_LOCAL;",
        "            Some(StandardSymbol::Type(StandardType::ProcessInbox)) => {\n                return ValueProperties::SHAREABLE;")]),
    # Interpreter and build-time evaluation.
    language("Process.spec accepts a computed zero capacity", I, [(
        "                if *capacity < 1 {\n                    Err(format!(\n                        \"Process.spec:",
        "                if *capacity < 0 {\n                    Err(format!(\n                        \"Process.spec:")]),
    language("Process.register overwrites an existing name", I, [(
        "                Some(RuntimeValue::Constructed { fields: named, .. }) if !named.is_empty() => {",
        "                Some(RuntimeValue::Constructed { fields: named, .. }) if named.len() > 1 => {")]),
    language("process runtime operations are pure at build time", K, [(
        "                Some(module) if function.requires_process_runtime() => {",
        "                Some(module) if false => {")]),
    # The static runner.
    lab("static runner ignores diagnostic columns", L, [(
        '.map(|((module, line, column, code), _)| format!("{code} {module}.chimi:{line}:{column}"))',
        '.map(|((module, line, _column, code), _)| format!("{code} {module}.chimi:{line}:1"))')]),
    # Review follow-ups.
    cli("native build accepts process programs", N, [(
        "    if requires_process_runtime(&program.core) {",
        "    if false && requires_process_runtime(&program.core) {")]),
    cli("native refusal misses a program that only names a process", N, [(
        "        symbol.kind == crate::executable::CoreSymbolKind::ProcessName\n",
        "        false\n")]),
    language("Supervisor.policy accepts more members than its computed capacity", I, [(
        "                } else if list.len() as i64 > *capacity {",
        "                } else if false {")]),
    language("T4262 piles onto a name resolution rejected", R, [(
        "                    && !self.reports_its_own_resolution(module_id, name)",
        "                    && true")]),
]
