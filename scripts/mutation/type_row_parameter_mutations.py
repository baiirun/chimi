"""Mutants of rejecting a type's needs row parameter (CHI-300, T4227).

Run: scripts/mutation/mutate.py scripts/mutation/type_row_parameter_mutations.py
"""

CRATE = "compiler/chimi"

TYPING = "src/typing.rs"


def mutant(name, edits):
    return dict(name=name, file=TYPING, edits=edits, targets=["language"], filter="a_type_parameter_cannot")


MUTATIONS = [
    mutant("a type's row parameter is accepted", [(
        "                self.reject_type_needs_row_parameters(\n",
        "                if false { self.reject_type_needs_row_parameters(\n",
    ), (
        "                    &generic_symbols,\n                );\n                if declaration.kind == ModuleDeclarationKind::Struct {",
        "                    &generic_symbols,\n                ); }\n                if declaration.kind == ModuleDeclarationKind::Struct {",
    )]),
    mutant("the members keep the rejected row", [(
        "        if rejected.needs.is_empty() {\n            return;\n        }",
        "        if true {\n            return;\n        }",
    )]),
    mutant("variant fields keep the rejected row", [(
        "        let mut shapes = vec![declaration_symbol];\n        shapes.extend(",
        "        let mut shapes = vec![declaration_symbol];\n        let _ = (",
    )]),
]
