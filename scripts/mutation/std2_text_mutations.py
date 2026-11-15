"""Mutants of the Unicode text work (case mapping, grapheme clusters, padding,
case-insensitive comparison): the interpreter in src/execution/core_operations.rs
and src/graphemes.rs, the native runtime.c, and std/text.chimi.

Run: scripts/mutation/mutate.py scripts/mutation/std2_text_mutations.py
Each mutant must turn tests/std_modules.rs (std-tests programs, both backends),
the graphemes lib tests, or tests/unicode_tables.rs red.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
CORE = "src/execution/core_operations.rs"
GRAPHEMES = "src/graphemes.rs"
TEXT = "std/text.chimi"


def mutant(name, file, edits, targets, filter=""):
    return dict(name=name, file=file, edits=edits, targets=targets, filter=filter)


def std(name, file, edits):
    return mutant(name, file, edits, ["std_modules"], "standard_programs")


def graphemes(name, file, edits):
    return mutant(name, file, edits, ["lib", "std_modules", "unicode_tables"], "graphemes")


MUTATIONS = [
    std("native uppercase uses only the first (simple) mapping", RUNTIME, [(
        "for (int i = 0; i < 3 && mapping->mapped[i]; ++i)",
        "for (int i = 0; i < (final_sigma ? 3 : 1) && mapping->mapped[i]; ++i)")]),
    std("interpreter lowercase never yields final sigma", CORE, [(
        "                text(value.to_lowercase())",
        "                text(value.to_lowercase().replace('\\u{3c2}', \"\\u{3c3}\"))")]),
    std("native Final_Sigma ignores a following cased letter", RUNTIME, [(
        "                      !cased_beyond_ignorables(text, offset + width, false);",
        "                      true;")]),
    std("native GB9 drops Extend", RUNTIME, [(
        "if (current == GB_EXTEND || current == GB_ZWJ ||",
        "if (current == GB_ZWJ ||")]),
    graphemes("interpreter GB11 drops ZWJ emoji sequences", GRAPHEMES, [(
        "(Zwj, _) if self.pictographic_joiner && next.pictographic => false,",
        "(Zwj, _) if self.pictographic_joiner && next.pictographic && false => false,")]),
    std("native GB12/GB13 breaks every regional indicator pair", RUNTIME, [(
        "current == GB_REGIONAL_INDICATOR) return !cluster->odd_regional_indicators;",
        "current == GB_REGIONAL_INDICATOR) return true;")]),
    graphemes("interpreter GB9c drops InCB conjuncts", GRAPHEMES, [(
        "_ if self.conjunct == ConjunctRun::Linked && next.conjunct == Conjunct::Consonant => {",
        "_ if self.conjunct == ConjunctRun::Linked && next.conjunct == Conjunct::Consonant && false => {")]),
    std("native splits CR LF", RUNTIME, [(
        "if (previous == GB_CR && current == GB_LF) return false;",
        "if (previous == GB_CR && current == GB_LF) return true;")]),
    std("pads text that is already wide enough", TEXT, [(
        "        True -> \"\"\n        False -> append(repeat(",
        "        True -> fill\n        False -> append(repeat(")]),
    std("pad fill is not cut by graphemes", TEXT, [(
        "concat(List.take(pieces, missing % size))",
        "\"\"")]),
    std("compare_ignoring_case only lowercases", TEXT, [(
        "  lowercase(uppercase(text))",
        "  lowercase(text)")]),
    std("grapheme_count of empty text is 1", TEXT, [(
        "      False -> break count\n    }\n  }\n}\n\n/// Returns `True` when `needle`",
        "      False -> break case length == 0 {\n        True -> 1\n        False -> count\n      }\n    }\n  }\n}\n\n/// Returns `True` when `needle`")]),
]
