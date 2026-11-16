"""Mutants of generic failure enums in failure sets (CHI-232): a variant
raised at two type arguments binding its fields, the whole error passing as
the declaration's type, and a written `W<Int>` member losing its argument.

Run: scripts/mutation/mutate.py scripts/mutation/generic_failure_mutations.py
"""

CRATE = "compiler/chimi"

TYPING = "src/typing.rs"
SETS = "src/typing/failure_sets.rs"


def language(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


MUTATIONS = [
    language("a pattern binds fields of a variant raised at two arguments", TYPING, [
        ("            && self.variant_raised_at_several_arguments(failures, constructor)\n        {\n            self.type_error_range(",
         "            && false\n        {\n            self.type_error_range(")],
        "a_variant_raised_at_two_type_arguments_cannot_bind_its_fields"),
    language("disagreeing arguments fall back to the declaration type", TYPING, [
        ("        if self.raised_at_known_arguments(&row) {", "        if false {")],
        "a_variant_raised_at_two_type_arguments_cannot_bind_its_fields"),
    language("poison disagrees with a known argument", TYPING, [
        ("                        left != right && *left != poison && *right != poison",
         "                        left != right")],
        "a_written_generic_failure_enum_keeps_its_type_arguments"),
    language("a written generic member drops its arguments", SETS, [
        ("        let variant_arguments = if !arguments.is_empty() && arguments.len() == declared {",
         "        let variant_arguments = if false {")],
        "a_written_generic_failure_enum_keeps_its_type_arguments"),
]
