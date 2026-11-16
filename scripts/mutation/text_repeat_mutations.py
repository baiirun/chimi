"""Text.repeat by doubling: each mutant must make a std test fail."""

CRATE = "compiler/chimi"

MUTATIONS = [
    dict(
        name="an odd count drops its last copy",
        file="std/text.chimi",
        edits=[("        let result = case n % 2 == 1 {", "        let result = case n % 2 == 2 {")],
        targets=["std_modules"],
    ),
    dict(
        name="the piece is not doubled between passes",
        file="std/text.chimi",
        edits=[("          half -> (half, append(piece, piece), result)", "          half -> (half, piece, result)")],
        targets=["std_modules"],
    ),
    dict(
        name="the loop stops one pass early",
        file="std/text.chimi",
        edits=[("        case n / 2 {\n          0 -> break result", "        case n / 4 {\n          0 -> break result")],
        targets=["std_modules"],
    ),
]
