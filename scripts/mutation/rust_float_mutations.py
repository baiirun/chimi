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
    fuzz("exponent sign ignored when read", FLOAT,
         [("    if negative { -value } else { value }", "    value")]),
    fuzz("no candidate above the nearest", FLOAT,
         [("        } else if round_trips(nearest + 1, scale, magnitude) {\n            nearest + 1\n",
           "        } else if false {\n            nearest + 1\n")]),
    fuzz("no candidate below the nearest", FLOAT,
         [("        } else if nearest > smallest && round_trips(nearest - 1, scale, magnitude) {",
           "        } else if false && nearest > smallest && round_trips(nearest - 1, scale, magnitude) {")]),
    fuzz("a tie keeps the smaller candidate", FLOAT,
         [("round_trips(nearest + 1, scale, magnitude) && is_tie(magnitude, nearest) { nearest + 1 }",
           "round_trips(nearest + 1, scale, magnitude) && false { nearest + 1 }")]),
    fuzz("a carry keeps its trailing zero", FLOAT,
         [("        let exponent = scale + digits.len() as i32 - 1;\n        digits.trim_end(b'0', 1);\n",
           "        let exponent = scale + digits.len() as i32 - 1;\n")]),
    fuzz("the 17-digit fallback keeps its zeros", FLOAT,
         [("        digits.push(byte);\n    }\n    digits.trim_end(b'0', 1);", "        digits.push(byte);\n    }")]),
    fuzz("C glue drops the text", RUNTIME,
         [("static void float_text(double value, char *output) { chimi_float_text_v1(value, output); }",
           "static void float_text(double value, char *output) { chimi_float_text_v1(value, output); output[0] = '0'; }")]),
]
