"""Mutants of the shared text code (CHI-307): runtime/chimi-runtime/src/
{text,graphemes}.rs and the native Text intrinsics over them in
intrinsics/core.rs (C glue in runtime.c until CHI-305 step 5 moved it).

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
CORE_INTRINSICS = RT + "intrinsics/core.rs"
RUNTIME = "runtime/native/runtime.c"


def fuzz(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_fuzz"], filter="")


MUTATIONS = [
    fuzz("ideographic space is not white space", TEXT,
         [("            | '\\u{205f}'\n            | '\\u{3000}'\n", "            | '\\u{205f}'\n")]),
    fuzz("no-break space is not white space", TEXT,
         [("            | '\\u{a0}'\n", "")]),
    fuzz("trim_end trims the start", CORE_INTRINSICS,
         [("{ text::trim_end(whole) } else { text::trim_start(whole) }",
           "{ text::trim_start(whole) } else { text::trim_start(whole) }")]),
    fuzz("uppercase lowercases", CORE_INTRINSICS,
         [("{ text::uppercase(source) } else { text::lowercase(source) }",
           "{ text::lowercase(source) } else { text::lowercase(source) }")]),
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
    fuzz("native trim keeps the start", CORE_INTRINSICS,
         [("success(new_text(kept.as_bytes()))",
           "success(new_text(&whole.as_bytes()[..kept.len()]))")]),
    fuzz("native case map drops the last byte", CORE_INTRINSICS,
         [("success(new_text(mapped.as_bytes()))",
           "success(new_text(&mapped.as_bytes()[..mapped.len().saturating_sub(1)]))")]),
    fuzz("native graphemes drop the last cluster", CORE_INTRINSICS,
         [("for cluster in graphemes::graphemes(whole) {",
           "for cluster in graphemes::graphemes(whole).take(graphemes::graphemes(whole).count().saturating_sub(1)) {")]),
]
