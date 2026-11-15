"""Mutants of generic interface members (G11, CHI-104): parsing `fn<A>` in a
function type, scoping member generics, the placement rule, substitution under
a member's own binder, and the conformance check for providers and updates.

Run: scripts/mutation/mutate.py scripts/mutation/generic_member_mutations.py
Each mutant runs tests/generic_interface_members.rs; it must turn one red.
"""

CRATE = "compiler/chimi"

Y = "src/typing.rs"
R = "src/resolution.rs"
P = "src/syntax.rs"
F = "src/typing/failure_sets.rs"


def member(name, file, edits, targets=("generic_interface_members",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter="")


MUTATIONS = [
    member("parser: a function type takes no generic parameters", P, [(
        "        self.bump();\n        self.parse_optional_generic_parameters();\n        self.skip_newlines();\n        self.expect(TokenKind::LeftParen);",
        "        self.bump();\n        self.skip_newlines();\n        self.expect(TokenKind::LeftParen);",
    )]),
    member("resolution: a member generic may shadow the interface's", R, [(
        "            if generics.contains_key(&name.text) {\n                let module = &self.semantic.modules[module_index];\n                self.diagnostics.push(name_error(\n                    module,\n                    \"E_DUPLICATE_GENERIC_PARAMETER\",\n                    name.range,\n                    format!(\"generic parameter {} is declared more than once\", name.text),\n                ));\n                continue;\n            }\n            let existing",
        "            let existing",
    )]),
    member("typing: a generic function type is allowed anywhere", Y, [(
        "                if let Some(first) = generic_parameters.first()\n                    && !generic_function_type_allowed\n",
        "                if let Some(first) = generic_parameters.first()\n                    && false\n",
    )]),
    member("typing: nested types inherit the member permission", Y, [(
        "let generic_function_type_allowed = std::mem::take(&mut self.generic_function_type_allowed);",
        "let generic_function_type_allowed = self.generic_function_type_allowed;",
    )]),
    member("typing: substitution leaves a generic member untouched", Y, [(
        "                let contract =\n                    self.substitute_call_contract_preserving(contract, &mut inner, &inner_preserved);\n                self.intern_type(CanonicalType::PolymorphicFunction {",
        "                let _ = (&mut inner, &inner_preserved);\n                self.intern_type(CanonicalType::PolymorphicFunction {",
    )]),
    member("typing: substitution freshens the member's own generics", Y, [(
        "                    inner.values.remove(parameter);\n",
        "",
    )], targets=("generic_interface_members", "language")),
    member("conformance: a generic slot unifies like a plain one", Y, [(
        "            ) => self.generic_member_conformance_error(actual, &parameters, contract),\n            (CanonicalType::Function(actual), CanonicalType::Function(expected)) => {",
        "            ) if false => self.generic_member_conformance_error(actual, &parameters, contract),\n            (CanonicalType::Function(actual), CanonicalType::Function(expected)) => {",
    )]),
    member("conformance: no escape check for existing variables", Y, [(
        "                bound != bindings.get(&variable).copied()\n                    && bound",
        "                false && bound",
    )]),
    member("conformance: no requirement check on member generics", Y, [(
        "            self.generic_requirements.contains_key(parameter)\n                && substitution",
        "            false\n                && substitution",
    )]),
    member("conformance: a failed check keeps its bindings", Y, [(
        "        if reason.is_some() {\n            self.inference_bindings = bindings;",
        "        if reason.is_some() && false {\n            self.inference_bindings = bindings;",
    )]),
    member("join: a generic and a fixed function join to the generic type", F, [(
        "            return Some(self.fixed_function_side(right, left));",
        "            return Some(right);",
    )]),
    member("call: a generic binding is kept after a fixed function matches it", Y, [(
        "                        substitution.values.insert(parameter, actual);\n                    }\n                    unified",
        "                    }\n                    unified",
    )]),
    member("lowering: member generics are not in scope in needs rows", Y, [(
        "                let generics = if parameters.is_empty() {\n                    generics\n                } else {\n                    scoped = generics.clone();",
        "                let generics = if true {\n                    generics\n                } else {\n                    scoped = generics.clone();",
    )]),
]
