"""Mutants of records fitting interface types (CHI-244): result, let and
argument checks, generic calls, joins, struct fields and the diagnostics.

Run: scripts/mutation/mutate.py scripts/mutation/interface_value_mutations.py
"""

CRATE = "compiler/chimi"

TYPING = "src/typing.rs"
FAILURE_SETS = "src/typing/failure_sets.rs"
FILTER = "a_record_"


def mutant(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=FILTER)


MUTATIONS = [
    mutant("results and lets ignore conformance", TYPING, [(
        "            Some(Ok(())) => return true,\n            Some(Err(reason)) => {",
        "            Some(Ok(())) => {}\n            Some(Err(reason)) => {",
    )]),
    mutant("any record fits an interface", TYPING, [(
        "        Some(match self.provider_conformance_error(actual, expected) {\n            None => Ok(()),",
        "        Some(match None::<String> {\n            None => Ok(()),",
    )]),
    mutant("the reason is dropped from the message", TYPING, [(
        "                    format!(\"{message}; the record {reason}\"),",
        "                    message,",
    )]),
    mutant("generic calls skip the interface rule", TYPING, [(
        "            if self.is_plain_interface_type(resolved_expected)\n                && let Some(conformance)",
        "            if false\n                && let Some(conformance)",
    )]),
    mutant("generic calls accept any record", TYPING, [(
        "                return conformance.is_ok();",
        "                return true || conformance.is_ok();",
    )]),
    mutant("a record never joins an interface", FAILURE_SETS, [(
        "        if let Some(Ok(())) = self.record_interface_conformance(left, right) {\n            return Some(right);\n        }\n        if let Some(Ok(())) = self.record_interface_conformance(right, left) {\n            return Some(left);\n        }\n",
        "",
    )]),
    mutant("a join keeps the record side", FAILURE_SETS, [(
        "        if let Some(Ok(())) = self.record_interface_conformance(right, left) {\n            return Some(left);",
        "        if let Some(Ok(())) = self.record_interface_conformance(right, left) {\n            return Some(right);",
    )]),
    mutant("struct fields and arguments skip the interface rule", TYPING, [(
        "            (CanonicalType::Record(_), CanonicalType::Named { .. })\n                if self.is_interface_type(expected) =>",
        "            (CanonicalType::Record(_), CanonicalType::Named { .. })\n                if false =>",
    )]),
    mutant("structs fit interfaces", TYPING, [(
        "        if !matches!(self.types[actual.raw() as usize], CanonicalType::Record(_))\n            || !self.is_interface_type(expected)",
        "        if !matches!(self.types[actual.raw() as usize], CanonicalType::Record(_) | CanonicalType::Named { .. })\n            || !self.is_interface_type(expected)",
    )]),
]
