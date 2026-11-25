"""Mutants of the shared text code (CHI-307): runtime/chimi-runtime/src/
{text,graphemes,text_abi}.rs and the C glue in runtime.c.

Both backends run this code, so the language tests that compare them cannot
see a mutant here; the runtime fuzz corpus replay (tests/runtime_fuzz.rs),
which checks each input against its recorded output and against the C code
the Rust replaced, must. Each mutant targets only that replay.

Run: scripts/mutation/mutate.py scripts/mutation/rust_text_mutations.py
"""

CRATE = "compiler/chimi"

RT = "../../runtime/chimi-runtime/src/"
TEXT = RT + "text.rs"
GRAPHEMES = RT + "graphemes.rs"
ABI = RT + "text_abi.rs"
RUNTIME = "runtime/native/runtime.c"


def fuzz(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_fuzz"], filter="")


MUTATIONS = [
    fuzz("ideographic space is not white space", TEXT,
         [("            | '\\u{205f}'\n            | '\\u{3000}'\n", "            | '\\u{205f}'\n")]),
    fuzz("no-break space is not white space", TEXT,
         [("            | '\\u{a0}'\n", "")]),
    fuzz("trim_end trims the start", ABI,
         [("let kept = if end { text::trim_end(whole) } else { text::trim_start(whole) };",
           "let kept = if end { text::trim_start(whole) } else { text::trim_start(whole) };")]),
    fuzz("uppercase lowercases", ABI,
         [("if upper { text::uppercase(source) } else { text::lowercase(source) }",
           "if upper { text::lowercase(source) } else { text::lowercase(source) }")]),
    fuzz("CR LF splits", GRAPHEMES,
         [("            (Cr, Lf) => false,\n", "")]),
    fuzz("spacing marks start a cluster", GRAPHEMES,
         [("(_, Extend | Zwj | SpacingMark) | (Prepend, _) => false,",
           "(_, Extend | Zwj) | (Prepend, _) => false,")]),
    fuzz("Hangul LV T splits", GRAPHEMES,
         [("(L, L | V | Lv | Lvt) | (Lv | V, V | T) | (Lvt | T, T) => false,",
           "(L, L | V | Lv | Lvt) | (Lv | V, V) | (Lvt | T, T) => false,")]),
    fuzz("regional indicators pair wrongly", GRAPHEMES,
         [("(RegionalIndicator, RegionalIndicator) => !self.odd_regional_indicators,",
           "(RegionalIndicator, RegionalIndicator) => self.odd_regional_indicators,")]),
    fuzz("emoji ZWJ sequences split", GRAPHEMES,
         [("            (Zwj, _) if self.pictographic_joiner && next.pictographic => false,\n", "")]),
    fuzz("conjuncts split", GRAPHEMES,
         [("_ if self.conjunct == ConjunctRun::Linked && next.conjunct == Conjunct::Consonant => {",
           "_ if false && next.conjunct == Conjunct::Consonant => {")]),
    fuzz("property lookup one range late", GRAPHEMES,
         [("table::RANGES.partition_point(|&(_, last, ..)| last < code);",
           "table::RANGES.partition_point(|&(_, last, ..)| last <= code);")]),
    fuzz("C trim keeps the start", RUNTIME,
         [("return text_from_bytes(value->as.text.bytes + kept.start, kept.length);",
           "return text_from_bytes(value->as.text.bytes, kept.length);")]),
    fuzz("C case map drops the last byte", RUNTIME,
         [("ChimiValue *result = text_from_bytes(mapped.bytes, mapped.length);",
           "ChimiValue *result = text_from_bytes(mapped.bytes, mapped.length - (mapped.length > 0));")]),
    fuzz("C graphemes drop the last cluster", RUNTIME,
         [("for (size_t index = 0, start = 0; index < ends.count; start = ends.ends[index++])",
           "for (size_t index = 0, start = 0; index + 1 < ends.count; start = ends.ends[index++])")]),
]
