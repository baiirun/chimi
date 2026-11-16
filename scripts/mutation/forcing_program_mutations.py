"""Mutants of the CHI-207 fix: `Result`'s type-level walks bind the runtime's
generics, which `Ok` and `Err` are written in.

Run: scripts/mutation/mutate.py scripts/mutation/forcing_program_mutations.py
Results: scripts/mutation/forcing_program_results.txt.
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
    dict(
        name="Result binds its value and error generics swapped",
        file=TYPING,
        edits=[(
            "            return [StandardGeneric::ResultValue, StandardGeneric::ResultError]",
            "            return [StandardGeneric::ResultError, StandardGeneric::ResultValue]",
        )],
        targets=["lib", "channel_variants"],
        filter="result",
    ),
]
