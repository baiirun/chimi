"""Mutants for three compiler fixes: outcome patterns in a multi-subject case
(ts-d017ce), function clauses that allocate what the equivalent case allocates
(ts-6c5e9e), and record updates that allocate what the written-out constructor
allocates (ts-8250de).

Run: scripts/mutation/mutate.py scripts/mutation/compiler_small_mutations.py
Results: compiler/chimi/tests/SMALL-FIXES-MUTATION-RESULTS.txt
"""

CRATE = "compiler/chimi"

T = "src/typing.rs"
E = "src/executable.rs"


def lang(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


def cli(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=filter)


MULTI = "multi_subject"
CLAUSES = "native_function_clauses_allocate_what_the_equivalent_case_allocates"
UPDATE_ALLOCATION = "native_record_update_allocates_what_the_written_out_constructor_allocates"

MUTATIONS = [
    # ts-d017ce
    lang("outcome case only for a single subject", T, [(
        "        if outcome_columns.contains(&true) {",
        "        if subjects.len() == 1 && outcome_columns[0] {",
    )], MULTI),
    lang("a later subject's failure must be covered even after an earlier failure", T, [(
        "                            std::cmp::Ordering::Less => true,",
        "                            std::cmp::Ordering::Less => bits >> later & 1 == 0,",
    ), (
        "            let signatures = (0..1_usize << later)",
        "            let signatures = (0..1_usize << (later + position))",
    )], MULTI),
    lang("subtract a variant whatever the other subjects hold", T, [(
        "                if fully_covered(self, candidate) {",
        "                if { let _ = fully_covered; true } {",
    )], MULTI),
    lang("accept a plain pattern in an outcome column silently", T, [(
        '                    self.type_error_range(module_id, "T4222", pattern.head_range(), message);',
        "                    let _ = message;",
    )], MULTI),
    # ts-6c5e9e
    cli("clause bindings stay bindings", E, [(
        "                        self.clause_parameter_aliases.insert(*binding, *parameter);\n"
        "                        *pattern = CorePattern::Wildcard;",
        "                        let _ = (binding, parameter);",
    )], CLAUSES),
    cli("keep every clause subject", E, [(
        "                !clauses\n                    || lowered",
        "                true\n                    || lowered",
    )], CLAUSES),
    cli("aliased clause bindings keep their slots", E, [(
        "        if lowerer.clause_parameter_aliases.contains_key(&symbol.id)\n"
        "            || lowerer.unbound_temporaries.contains(&symbol.id)",
        "        if lowerer.unbound_temporaries.contains(&symbol.id)",
    )], CLAUSES),
    # ts-8250de
    cli("always bind the update base", E, [(
        "                .filter(|symbol| self.is_local(*symbol)),",
        "                .filter(|_| false),",
    )], UPDATE_ALLOCATION),
    cli("always bind the update values", E, [(
        "        let in_order = replacements.windows(2).all(|pair| pair[0] < pair[1]);",
        "        let in_order = replacements.is_empty();",
    )], UPDATE_ALLOCATION),
    cli("unbound update temporaries keep their slots", E, [(
        "        if lowerer.clause_parameter_aliases.contains_key(&symbol.id)\n"
        "            || lowerer.unbound_temporaries.contains(&symbol.id)",
        "        if lowerer.clause_parameter_aliases.contains_key(&symbol.id)",
    )], UPDATE_ALLOCATION),
    lang("inline update values out of declaration order", E, [(
        "        let in_order = replacements.windows(2).all(|pair| pair[0] < pair[1]);",
        "        let in_order = { let _ = &replacements; true };",
    )], "update"),
]
