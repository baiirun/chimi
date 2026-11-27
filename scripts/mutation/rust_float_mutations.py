"""Mutants of the shared Float text (CHI-307): runtime/chimi-runtime/src/float.rs
and its C glue in runtime.c.

Both backends print with this code, so comparing them cannot see a mutant
here; the runtime fuzz corpus replay (tests/runtime_fuzz.rs), which checks each
value against its recorded text and against the C code the Rust replaced, must.
Each mutant targets only that replay.

Run: scripts/mutation/mutate.py scripts/mutation/rust_float_mutations.py
"""

CRATE = "compiler/chimi"

FLOAT = "../../runtime/chimi-runtime/src/float.rs"
RUNTIME = "runtime/native/runtime.c"


def fuzz(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_fuzz"], filter="")


def digits(name, edits):
    """A mutant of the digit choice, which the corpus samples too sparsely to
    pin; tests/float_text.rs compares it with core's shortest digits."""
    return dict(name=name, file=FLOAT, edits=edits, targets=["float_text"], filter="")


MUTATIONS = [
    fuzz("positional up to exponent 16", FLOAT,
         [("if (-5..16).contains(&exponent) {", "if (-5..17).contains(&exponent) {")]),
    fuzz("positional from exponent -4", FLOAT,
         [("if (-5..16).contains(&exponent) {", "if (-4..16).contains(&exponent) {")]),
    fuzz("negative zero loses its sign", FLOAT,
         [("    if value.is_sign_negative() {\n        text.push(b'-');\n    }\n    if value == 0.0 {",
           "    if value.is_sign_negative() && value != 0.0 {\n        text.push(b'-');\n    }\n    if value == 0.0 {")]),
    fuzz("one zero short after the point", FLOAT,
         [("for _ in 0..(-exponent - 1) {", "for _ in 0..(-exponent - 2) {")]),
    fuzz("whole numbers lose their .0", FLOAT,
         [("                text.push_str(\".0\");\n", "")]),
    fuzz("single digits keep a point", FLOAT,
         [("        if digits.len() > 1 {\n            text.push(b'.');",
           "        {\n            text.push(b'.');")]),
    fuzz("negative exponents lose their sign", FLOAT,
         [("        if exponent < 0 {\n            text.push(b'-');\n        }\n        text.push_digits",
           "        text.push_digits")]),
    digits("ties round down",
           [("r.sum(&r).order(&s).is_ge()", "r.sum(&r).order(&s).is_gt()")]),
    digits("even significands exclude the interval's ends",
           [("let inclusive = significand % 2 == 0;", "let inclusive = false;")]),
    digits("a power of two keeps an even interval",
           [("if biased > 1 && fraction == 0 {", "if false && biased > 1 && fraction == 0 {")]),
    digits("the low end is open for even significands",
           [("if inclusive { r.order(&m_minus).is_le() }", "if inclusive { r.order(&m_minus).is_lt() }")]),
    digits("the estimate is never stepped up",
           [("    while beyond_top(&r, &m_plus, &s, inclusive) {\n", "    while false && beyond_top(&r, &m_plus, &s, inclusive) {\n")]),
    digits("rounding up keeps the last digit",
           [("            digits[count - 1] += 1;\n", "")]),
    digits("every value takes the u128 path",
           [("if s.length < 4 || (s.length == 4 && s.limbs[3] < 1 << 24) {", "if true {")]),
    digits("big subtraction drops the borrow",
           [("borrow = i64::from(difference < 0);", "borrow = 0;")]),
    digits("big shifts drop the carried bits",
           [("carry = *limb >> (32 - bits);", "carry = 0;")]),
    digits("big sums drop the carry",
           [("            sum.limbs[length] = 1;\n", "")]),
    fuzz("C glue drops the text", RUNTIME,
         [("static void float_text(double value, char *output) { chimi_float_text_v1(value, output); }",
           "static void float_text(double value, char *output) { chimi_float_text_v1(value, output); output[0] = '0'; }")]),
]
