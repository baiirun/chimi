"""Mutants of the CHI-207 fix: `Result`'s type-level walks bind the runtime's
generics, which `Ok` and `Err` are written in.

Run: scripts/mutation/mutate.py scripts/mutation/forcing_program_mutations.py
Results: scripts/mutation/forcing_program_results.txt.

Left out as equivalent: binding the two generics in swapped order. Every walk
that reads them (value properties, `type_may_hold`, constant representability)
combines all of a type's fields, so the order cannot change an answer.
"""

CRATE = "compiler/chimi"

TYPING = "src/typing.rs"

MUTATIONS = [
    dict(
        name="Result answers its source declaration's generics",
        file=TYPING,
        edits=[(
            "            == Some(StandardSymbol::Type(StandardType::Result))\n        {",
            "            == Some(StandardSymbol::Type(StandardType::Result)) && false\n        {",
        )],
        targets=["lib", "channel_variants"],
        filter="result",
    ),
]
