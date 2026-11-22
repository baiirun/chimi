"""Mutants of `using` with a provider record value (Phase 7c PR 6, G23):
the parser, lowering, typing, capability and core lowering.

Run: scripts/mutation/mutate.py scripts/mutation/using_record_mutations.py
"""

CRATE = "compiler/chimi"

SYNTAX = "src/syntax.rs"
SEMANTIC = "src/semantic.rs"
TYPING = "src/typing.rs"
CAPABILITY = "src/capability.rs"
EXECUTABLE = "src/executable.rs"


def mutant(name, file, edits, filter="", targets=("language",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("the parser wants a literal after using", SYNTAX, [(
        "            if self.at(TokenKind::LeftBrace) {\n                self.parse_record();",
        "            if true {\n                self.parse_record();",
    )]),
    mutant("the parser takes a whole expression after using", SYNTAX, [(
        "            } else {\n                self.parse_postfix();\n            }",
        "            } else {\n                self.parse_or();\n            }",
    )]),
    mutant("lowering treats a value as an empty literal", SEMANTIC, [(
        "            if record.kind() != NodeKind::RecordExpression\n                || direct_child",
        "            if false\n                && direct_child",
    )]),
    mutant("a record value supplies nothing", TYPING, [(
        "        self.supply_providers(module_id, expression_fact, &supplied, vec![record_fact], rejected)",
        "        self.supply_providers(module_id, expression_fact, &[], vec![record_fact], rejected)",
    )]),
    mutant("an unknown type reads as a wrong type", TYPING, [(
        "            CanonicalType::InferenceVariable(_) => {\n                self.type_error_range(\n                    module_id,\n                    \"T4218\",",
        "            CanonicalType::InferenceVariable(_) if false => {\n                self.type_error_range(\n                    module_id,\n                    \"T4218\",",
    )]),
    mutant("a record value's own needs are dropped", TYPING, [(
        "        self.supply_providers(module_id, expression_fact, &supplied, vec![record_fact], rejected)",
        "        self.supply_providers(module_id, expression_fact, &supplied, Vec::new(), rejected)",
    )]),
    mutant("a record value under a spec is not checked", CAPABILITY, [(
        "                    if inner_spec_needs.remove(name) && nonshareable {",
        "                    if false {",
    )]),
    mutant("core lowering provides the whole record for each name", EXECUTABLE, [(
        "                CoreField { name, value }\n            })\n            .collect();\n        let provision = CoreOperation::Provision {",
        "                let _ = value;\n                CoreField { name, value: receiver }\n            })\n            .collect();\n        let provision = CoreOperation::Provision {",
    )]),
    mutant("a spread provider record keeps only its written fields", SEMANTIC, [(
        "                || direct_child(&record, NodeKind::RecordSpread).is_some()\n",
        "",
    )]),
    mutant("a rejected provider record explains nothing", TYPING, [(
        "        if explain_all {\n            explained_names.extend(",
        "        if false {\n            explained_names.extend(",
    )]),
]
