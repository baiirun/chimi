"""Mutants of the complete-needs rule (Phase 7c, CHI-289, T4204): a written
`needs` clause must list every need its body reaches.

Run: scripts/mutation/mutate.py scripts/mutation/complete_needs_mutations.py
"""

CRATE = "compiler/chimi"

T = "src/typing.rs"


def mutant(name, edits, filter="needs_clause", targets=("language",)):
    return dict(name=name, file=T, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("the check never runs", [(
        "        typed.validate_written_needs(body_modules);\n",
        "",
    )]),
    mutant("a function without a clause is checked too", [(
        "                    || signature.requirements.terms.is_empty()\n",
        "",
    )]),
    mutant("runner-closed needs are exempt", [(
        "                    if need.explained\n",
        "                    if need.explained || need.lexical_name == \"clock\"\n",
    )], filter="written"),
    mutant("only the first missing need is reported", [(
        "                    findings.push((\n                        module.source.file_id(),\n                        declaration.name.range,\n                        format!(\n                            \"{} writes its needs, so the list must be complete: add `{}: {}`, \\\n",
        "                    if findings.iter().any(|f: &(_, _, String)| f.0 == module.source.file_id()) { continue; }\n                    findings.push((\n                        module.source.file_id(),\n                        declaration.name.range,\n                        format!(\n                            \"{} writes its needs, so the list must be complete: add `{}: {}`, \\\n",
    )], filter="written_needs_clause_must"),
    mutant("any written entry excuses every need", [(
        "                    || written.contains(&need.lexical_name.as_str())\n",
        "                    || !written.is_empty()\n",
    )], filter="written_needs_clause_must"),
    mutant("an unwritten row parameter is ignored", [(
        "                    if owned && !written.contains(&parameter.name.as_str()) {",
        "                    if false && owned {",
    )], filter="reached_row_parameter"),
]
