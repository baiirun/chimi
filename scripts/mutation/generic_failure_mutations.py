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
        ("            && let Some(field) = self.field_with_several_types(failures, constructor, arguments)\n",
         "            && let Some(field) = None::<usize>\n")],
        "a_variant_raised_at_two_type_arguments_cannot_bind_its_fields"),
    language("disagreeing arguments fall back to the declaration type", TYPING, [
        ("        if self.raised_at_known_arguments(&row) {", "        if false {")],
        "a_variant_raised_at_two_type_arguments_cannot_bind_its_fields"),
    language("a field poison disagrees with a known type", TYPING, [
        ("                        left[*index] != right[*index]\n                            && left[*index] != poison\n                            && right[*index] != poison",
         "                        left[*index] != right[*index]")],
        "a_variant_raised_at_two_type_arguments_cannot_bind_its_fields"),
    language("a written generic member drops its arguments", SETS, [
        ("        let variant_arguments = if !arguments.is_empty() && arguments.len() == declared {",
         "        let variant_arguments = if false {")],
        "a_written_generic_failure_enum_keeps_its_type_arguments"),
    language("a wildcard field still counts as inspected", TYPING, [
        ("            !matches!(arguments.get(*index), None | Some(Pattern::Wildcard { .. }))",
         "            arguments.get(*index).is_some()")],
        "a_variant_raised_at_two_type_arguments_cannot_bind_its_fields"),
]
