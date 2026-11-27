"""Mutants of std/decimal (c08a24d): the Rust model in src/decimal.rs, its C
twin in runtime/native/runtime.c, the Chimi wrappers in std/decimal.chimi, and
the operator typing in typing.rs. Rust and C sides are mutated separately
because std_modules compares both backends against the same expected files.

Run: scripts/mutation/mutate.py --revision c95ed3b scripts/mutation/std2_decimal_mutations.py
Each mutant must turn the decimal unit tests, a std-tests/decimal program (both
backends), or a language.rs decimal test red.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
DEBUG_RENDER = "../../runtime/chimi-runtime/src/intrinsics/debug.rs"
MODEL = "src/decimal.rs"
TYPING = "src/typing.rs"
WRAPPER = "std/decimal.chimi"


def std(name, file, edits):
    # Rust model: the decimal unit tests (lib) and the std-tests programs.
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter="decimal")


def programs(name, file, edits):
    # C runtime and wrappers: only the std-tests programs compare backends.
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_programs")


def lang(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter="decimal")


MUTATIONS = [
    std("Rust magnitude bound allows 10^38 (39 digits)", MODEL, [(
        "const MAX_MAGNITUDE: u128 = 10u128.pow(MAX_SCALE) - 1;",
        "const MAX_MAGNITUDE: u128 = 10u128.pow(MAX_SCALE);")]),
    programs("C magnitude bound allows 10^38 (39 digits)", RUNTIME, [(
        "decimal_power(DECIMAL_MAX_SCALE, &power); return power - 1;",
        "decimal_power(DECIMAL_MAX_SCALE, &power); return power;")]),
    std("Rust HalfTowardZero rounds a tie away", MODEL, [(
        "Rounding::HalfTowardZero => remainder == Remainder::AboveHalf,",
        "Rounding::HalfTowardZero => remainder >= Remainder::Half,")]),
    programs("C HalfEven rounds a tie to an odd digit", RUNTIME, [(
        "(remainder == DECIMAL_HALF && quotient % 2 == 1)",
        "(remainder == DECIMAL_HALF && quotient % 2 == 0)")]),
    std("Rust Floor ignores the sign", MODEL, [(
        "Rounding::Floor => negative && inexact,",
        "Rounding::Floor => inexact,")]),
    programs("C Ceiling ignores the sign", RUNTIME, [(
        "case ROUND_CEILING: away = !negative && inexact; break;",
        "case ROUND_CEILING: away = inexact; break;")]),
    std("Rust equality also compares scale", MODEL, [(
        "Ordering::Equal => self.magnitude_cmp(other),",
        "Ordering::Equal => self.magnitude_cmp(other).then(self.scale.cmp(&other.scale)),")]),
    programs("C equality also compares scale", RUNTIME, [(
        "  return left.negative ? decimal_magnitude_compare(right, left) : decimal_magnitude_compare(left, right);",
        "  int by_value = left.negative ? decimal_magnitude_compare(right, left) : decimal_magnitude_compare(left, right);\n"
        "  return by_value != 0 ? by_value : (left.scale > right.scale) - (left.scale < right.scale);")]),
    programs("Rust to_text drops trailing zeros", "src/execution/decimal_operations.rs", [(
        "RuntimeValue::Text(value.to_text())",
        "RuntimeValue::Text(value.normalized_text())")]),
    lang("native Debug.render keeps trailing zeros", DEBUG_RENDER, [(
        "self.text(decimal.normalized().text().as_bytes());",
        "self.text(decimal.text().as_bytes());")]),
    programs("C multiply skips the overflow check", RUNTIME, [(
        "if (__builtin_mul_overflow(left.magnitude, right.magnitude, &product)) return false;",
        "product = left.magnitude * right.magnitude;")]),
    programs("Decimal.divide by zero returns zero instead of defecting", WRAPPER, [(
        "True -> Debug.crash(\"Decimal.divide: divisor must not be zero\")",
        "True -> zero")]),
    lang("typing accepts % on Decimal", TYPING, [(
        "if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder)\n            && decimal.is_some_and(",
        "if matches!(operator, BinaryOperator::Divide)\n            && decimal.is_some_and(")]),
]
