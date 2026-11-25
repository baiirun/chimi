"""Mutants of the shared decimal and duration models (CHI-307):
runtime/chimi-runtime/src/{decimal,duration,text_buffer}.rs, their C ABI in
decimal_abi.rs and duration_abi.rs, and the C glue in runtime.c.

Both backends now compute with the same Rust code, so the language tests that
compare the interpreter against the native runtime no longer see a model
mutant. What must see it is the runtime fuzz corpus replay
(tests/runtime_fuzz.rs), which checks every committed input against its
recorded output and against the C code the Rust replaced. Each mutant here
targets only that replay, so a kill shows the oracle covers the model.

Run: scripts/mutation/mutate.py scripts/mutation/rust_decimal_duration_mutations.py
"""

CRATE = "compiler/chimi"

RT = "../../runtime/chimi-runtime/src/"
DECIMAL = RT + "decimal.rs"
DECIMAL_ABI = RT + "decimal_abi.rs"
DURATION = RT + "duration.rs"
DURATION_ABI = RT + "duration_abi.rs"
TEXT = RT + "text_buffer.rs"
RUNTIME = "runtime/native/runtime.c"


def fuzz(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_fuzz"], filter="")


MUTATIONS = [
    fuzz("decimal magnitude bound allows 39 digits", DECIMAL,
         [("const MAX_MAGNITUDE: u128 = 10u128.pow(MAX_SCALE) - 1;",
           "const MAX_MAGNITUDE: u128 = 10u128.pow(MAX_SCALE);")]),
    fuzz("decimal fraction printed one digit short", DECIMAL,
         [("text.push_digits(self.magnitude % power, self.scale as usize);",
           "text.push_digits(self.magnitude % power, (self.scale as usize).saturating_sub(1));")]),
    fuzz("decimal read accepts a bare point", DECIMAL,
         [("fraction.is_some_and(|fraction| fraction.is_empty() || !digits_only(fraction))",
           "fraction.is_some_and(|fraction| !digits_only(fraction))")]),
    fuzz("half-even rounds a tie to odd", DECIMAL,
         [("|| (remainder == Remainder::Half && quotient % 2 == 1)",
           "|| (remainder == Remainder::Half && quotient % 2 == 0)")]),
    fuzz("normalized text keeps one trailing zero", DECIMAL,
         [("while value.scale > 0 && value.magnitude.is_multiple_of(10) {",
           "while value.scale > 1 && value.magnitude.is_multiple_of(10) {")]),
    fuzz("order past u128 reversed", DECIMAL,
         [("aligned(other, self.scale).map_or(Ordering::Less,",
           "aligned(other, self.scale).map_or(Ordering::Greater,")]),
    fuzz("division drops the divisor's scale", DECIMAL,
         [("let shift = i64::from(scale) + i64::from(divisor.scale) - i64::from(self.scale);",
           "let shift = i64::from(scale) - i64::from(self.scale);")]),
    fuzz("ABI compare reports less as greater", DECIMAL_ABI,
         [("Ordering::Less => -1,", "Ordering::Less => 1,")]),
    fuzz("ABI ignores the normalized flag", DECIMAL_ABI,
         [("let text = if normalized {", "let text = if false {")]),
    fuzz("ABI subtract adds", DECIMAL_ABI,
         [("unsafe { write(decimal(left).subtract(decimal(right)), out) }",
           "unsafe { write(decimal(left).add(decimal(right)), out) }")]),
    fuzz("ABI rounding codes shifted", DECIMAL_ABI,
         [("Rounding::from_code(i64::from(rounding))", "Rounding::from_code(i64::from(rounding).min(5))")]),
    fuzz("padding one zero short", TEXT,
         [("for _ in count..width {", "for _ in count + 1..width {")]),
    fuzz("duration fraction printed one digit short", DURATION,
         [("text.push_digits(u128::from(fraction), scale.ilog10() as usize);",
           "text.push_digits(u128::from(fraction), scale.ilog10() as usize - 1);")]),
    fuzz("duration digits read in base 9", DURATION,
         [(".fold(0, |value, digit| value * 10 + i128::from(digit - b'0'))",
           ".fold(0, |value, digit| value * 9 + i128::from(digit - b'0'))")]),
    fuzz("duration allows a repeated unit", DURATION,
         [(".is_some_and(|previous| previous.unit.nanoseconds() <= unit.nanoseconds())",
           ".is_some_and(|previous| previous.unit.nanoseconds() < unit.nanoseconds())")]),
    fuzz("duration rounds an inexact fraction", DURATION,
         [("        if numerator % denominator != 0 {\n            return Err(ParseFailure::Invalid);\n        }\n",
           "")]),
    fuzz("ABI parse reports Empty as Invalid", DURATION_ABI,
         [("Err(ParseFailure::Empty) => 1,", "Err(ParseFailure::Empty) => 2,")]),
    fuzz("C checked subtract adds", RUNTIME,
         [("CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED ? chimi_decimal_subtract_v1(left, right, &output)",
           "CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED ? chimi_decimal_add_v1(left, right, &output)")]),
    fuzz("C maps OutOfRange to Invalid", RUNTIME,
         [("CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID,\n                                        CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE};",
           "CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID,\n                                        CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID};")]),
    fuzz("C reads the text one byte short", RUNTIME,
         [("chimi_decimal_parse_v1(arguments[0]->as.text.bytes, (size_t)arguments[0]->as.text.length,",
           "chimi_decimal_parse_v1(arguments[0]->as.text.bytes, (size_t)arguments[0]->as.text.length - (arguments[0]->as.text.length > 0),")]),
]
