//! The numeric model shared by constant folding and the reference interpreter.
//!
//! `runtime/native/runtime.c` reimplements every function here. Both paths must
//! produce the same values and the same defect messages, so a change here needs
//! the matching change there.
//!
//! Invariant: every `f64` that enters or leaves this module is finite. A result
//! that would be NaN or an infinity is a defect instead.

use crate::semantic::BinaryOperator;

/// An operator that computes a number from two numbers of the same type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArithmeticOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

impl ArithmeticOperator {
    pub(crate) const fn from_binary(operator: BinaryOperator) -> Option<Self> {
        match operator {
            BinaryOperator::Add => Some(Self::Add),
            BinaryOperator::Subtract => Some(Self::Subtract),
            BinaryOperator::Multiply => Some(Self::Multiply),
            BinaryOperator::Divide => Some(Self::Divide),
            BinaryOperator::Remainder => Some(Self::Remainder),
            _ => None,
        }
    }

    pub(crate) const fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Remainder => "%",
        }
    }
}

/// Applies `operator` to two `Int` values.
///
/// Ensures: `Err` holds the defect message, which names the operator and both
/// operands; `%` truncates (the sign follows `left`) and `Int.min % -1` is 0.
pub(crate) fn integer_binary(
    operator: ArithmeticOperator,
    left: i64,
    right: i64,
) -> Result<i64, String> {
    let symbol = operator.symbol();
    let result = match operator {
        ArithmeticOperator::Add => left.checked_add(right),
        ArithmeticOperator::Subtract => left.checked_sub(right),
        ArithmeticOperator::Multiply => left.checked_mul(right),
        ArithmeticOperator::Divide | ArithmeticOperator::Remainder if right == 0 => {
            return Err(format!(
                "integer division by zero in `{symbol}`: {left} {symbol} {right}"
            ));
        }
        ArithmeticOperator::Divide => left.checked_div(right),
        // `checked_rem` reports `Int.min % -1` as overflow, but the remainder is exactly 0.
        ArithmeticOperator::Remainder => Some(if right == -1 { 0 } else { left % right }),
    };
    result.ok_or_else(|| format!("integer overflow in `{symbol}`: {left} {symbol} {right}"))
}

pub(crate) fn integer_negate(value: i64) -> Result<i64, String> {
    value
        .checked_neg()
        .ok_or_else(|| format!("integer overflow in unary `-`: -({value})"))
}

/// `Int.modulo(value, modulus)`: the remainder with the sign of `modulus`.
pub(crate) fn integer_modulo(value: i64, modulus: i64) -> Result<i64, String> {
    if modulus == 0 {
        return Err(format!(
            "integer division by zero in `Int.modulo`: Int.modulo({value}, {modulus})"
        ));
    }
    let remainder = value.wrapping_rem(modulus);
    Ok(if remainder != 0 && (remainder < 0) != (modulus < 0) {
        remainder + modulus
    } else {
        remainder
    })
}

/// Applies `operator` to two finite `Float` values.
///
/// Ensures: `Ok` is finite; `/ 0.0` and `% 0.0` are defects even when the
/// dividend is 0; `%` is C `fmod`.
pub(crate) fn float_binary(
    operator: ArithmeticOperator,
    left: f64,
    right: f64,
) -> Result<f64, String> {
    debug_assert!(left.is_finite() && right.is_finite());
    let symbol = operator.symbol();
    let operands = format!("{} {symbol} {}", render_float(left), render_float(right));
    let result = match operator {
        ArithmeticOperator::Add => left + right,
        ArithmeticOperator::Subtract => left - right,
        ArithmeticOperator::Multiply => left * right,
        ArithmeticOperator::Divide | ArithmeticOperator::Remainder if right == 0.0 => {
            return Err(format!("float division by zero in `{symbol}`: {operands}"));
        }
        ArithmeticOperator::Divide => left / right,
        // Rust's `%` on f64 is C `fmod`: truncated, with the sign of `left`.
        ArithmeticOperator::Remainder => left % right,
    };
    if result.is_finite() {
        Ok(result)
    } else {
        Err(format!("float `{symbol}` has no finite result: {operands}"))
    }
}

/// The canonical text of a finite `Float` (`chimi_runtime::float`), which
/// the native runtime prints too.
pub(crate) fn render_float(value: f64) -> String {
    String::from(chimi_runtime::float::text(value).as_str())
}

/// The value of a lexically valid float literal, `None` when it rounds to an infinity.
pub(crate) fn float_literal_value(text: &str) -> Option<f64> {
    let value: f64 = text.replace('_', "").parse().ok()?;
    value.is_finite().then_some(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntParseError {
    Empty,
    InvalidDigit,
    OutOfRange,
}

/// `Int.parse`: an optional `-` and decimal digits, nothing else.
pub(crate) fn parse_int(text: &str) -> Result<i64, IntParseError> {
    if text.is_empty() {
        return Err(IntParseError::Empty);
    }
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(IntParseError::InvalidDigit);
    }
    text.parse().map_err(|_| IntParseError::OutOfRange)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FloatParseError {
    Empty,
    Invalid,
    OutOfRange,
}

/// `Float.parse`: the decimal literal grammar without `_`, with an optional leading `-`.
///
/// Must not: accept whitespace, `+`, `nan`, `inf`, `1.`, or `.5`.
pub(crate) fn parse_float(text: &str) -> Result<f64, FloatParseError> {
    if text.is_empty() {
        return Err(FloatParseError::Empty);
    }
    if !is_decimal_number(text.strip_prefix('-').unwrap_or(text)) {
        return Err(FloatParseError::Invalid);
    }
    let value: f64 = text.parse().map_err(|_| FloatParseError::Invalid)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(FloatParseError::OutOfRange)
    }
}

/// `digits [. digits] [(e|E) [+|-] digits]`.
fn is_decimal_number(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    let digits = |index: &mut usize| {
        let start = *index;
        while *index < bytes.len() && bytes[*index].is_ascii_digit() {
            *index += 1;
        }
        *index > start
    };
    if !digits(&mut index) {
        return false;
    }
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        if !digits(&mut index) {
            return false;
        }
    }
    if index < bytes.len() && matches!(bytes[index], b'e' | b'E') {
        index += 1;
        if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        if !digits(&mut index) {
            return false;
        }
    }
    index == bytes.len()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FloatToIntError {
    NotInteger,
    OutOfRange,
}

/// `Float.to_int`: exact conversion of an integral value inside `Int`.
pub(crate) fn float_to_int(value: f64) -> Result<i64, FloatToIntError> {
    if value.fract() != 0.0 {
        return Err(FloatToIntError::NotInteger);
    }
    // -2^63 is exactly representable; 2^63 is the first value above `Int`.
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&value) {
        return Err(FloatToIntError::OutOfRange);
    }
    Ok(value as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remainder_truncates_and_int_min_by_minus_one_is_zero() {
        let rem = |a, b| integer_binary(ArithmeticOperator::Remainder, a, b);
        assert_eq!(rem(-7, 2), Ok(-1));
        assert_eq!(rem(7, -2), Ok(1));
        assert_eq!(rem(i64::MIN, -1), Ok(0));
        assert!(rem(5, 0).unwrap_err().contains("division by zero in `%`"));
        assert!(
            integer_binary(ArithmeticOperator::Divide, i64::MIN, -1)
                .unwrap_err()
                .contains("overflow in `/`")
        );
    }

    #[test]
    fn modulo_takes_the_sign_of_the_modulus() {
        assert_eq!(integer_modulo(-7, 3), Ok(2));
        assert_eq!(integer_modulo(7, -3), Ok(-2));
        assert_eq!(integer_modulo(-6, 3), Ok(0));
        assert_eq!(integer_modulo(i64::MIN, -1), Ok(0));
        assert_eq!(integer_modulo(i64::MIN, i64::MAX), Ok(i64::MAX - 1));
        assert!(integer_modulo(1, 0).is_err());
    }

    #[test]
    fn float_text_is_canonical_and_round_trips() {
        let cases = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (0.1, "0.1"),
            (0.30000000000000004, "0.30000000000000004"),
            (1234.5, "1234.5"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e16"),
            (0.00001, "0.00001"),
            (0.000001, "1e-6"),
            (2.5e-7, "2.5e-7"),
            (-6.02e23, "-6.02e23"),
            (f64::MAX, "1.7976931348623157e308"),
            (f64::from_bits(1), "5e-324"),
        ];
        for (value, text) in cases {
            assert_eq!(render_float(value), text);
            assert_eq!(parse_float(text).map(f64::to_bits), Ok(value.to_bits()));
        }
    }

    #[test]
    fn float_text_round_trips_for_sampled_bit_patterns() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..200_000 {
            // xorshift64: deterministic samples spread over every exponent.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state);
            if !value.is_finite() {
                continue;
            }
            let text = render_float(value);
            assert!(text.contains('.') || text.contains('e'), "{text}");
            assert_eq!(
                parse_float(&text).map(f64::to_bits),
                Ok(value.to_bits()),
                "{text}"
            );
        }
    }

    #[test]
    fn parsing_rejects_text_outside_the_literal_grammar() {
        assert_eq!(parse_int("42"), Ok(42));
        assert_eq!(parse_int("-9223372036854775808"), Ok(i64::MIN));
        assert_eq!(parse_int(""), Err(IntParseError::Empty));
        for invalid in ["4x", "+1", " 1", "-", "1_000", "0x1F"] {
            assert_eq!(
                parse_int(invalid),
                Err(IntParseError::InvalidDigit),
                "{invalid}"
            );
        }
        assert_eq!(
            parse_int("99999999999999999999"),
            Err(IntParseError::OutOfRange)
        );

        assert_eq!(parse_float("-2.5e-3"), Ok(-0.0025));
        assert_eq!(parse_float("7"), Ok(7.0));
        assert_eq!(parse_float(""), Err(FloatParseError::Empty));
        for invalid in ["nan", "inf", "+1.0", "1.", ".5", " 1.0", "1e", "1_0.0", "-"] {
            assert_eq!(
                parse_float(invalid),
                Err(FloatParseError::Invalid),
                "{invalid}"
            );
        }
        assert_eq!(parse_float("1e400"), Err(FloatParseError::OutOfRange));
        assert_eq!(parse_float("1e-400"), Ok(0.0));
    }

    #[test]
    fn float_to_int_requires_an_integral_value_inside_int() {
        assert_eq!(float_to_int(2.5), Err(FloatToIntError::NotInteger));
        assert_eq!(float_to_int(1e20), Err(FloatToIntError::OutOfRange));
        assert_eq!(
            float_to_int(9_223_372_036_854_775_808.0),
            Err(FloatToIntError::OutOfRange)
        );
        assert_eq!(float_to_int(-9_223_372_036_854_775_808.0), Ok(i64::MIN));
        assert_eq!(float_to_int(-0.0), Ok(0));
    }

    #[test]
    fn float_operations_defect_instead_of_leaving_the_finite_domain() {
        assert!(float_binary(ArithmeticOperator::Multiply, 1e300, 1e300).is_err());
        assert!(float_binary(ArithmeticOperator::Divide, 0.0, 0.0).is_err());
        assert!(float_binary(ArithmeticOperator::Remainder, 1.0, 0.0).is_err());
        assert_eq!(
            float_binary(ArithmeticOperator::Remainder, -7.5, 2.0),
            Ok(-1.5)
        );
        assert_eq!(
            float_binary(ArithmeticOperator::Subtract, 0.0, 0.0).map(f64::to_bits),
            Ok(0.0_f64.to_bits())
        );
    }
}
