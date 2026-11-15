//! The `std/decimal` number model shared by the reference interpreter and
//! constant evaluation: an exact signed coefficient of at most 38 decimal digits
//! and a scale from 0 to 38 (`core-data-and-operators.md`, "Decimal").
//!
//! `runtime/native/runtime.c` reimplements every function here (`decimal_*`).
//! Both paths must produce the same values, texts, and defect messages, so a
//! change here needs the matching change there; the `std-tests/decimal`
//! programs run on both backends.
//!
//! Invariant: no operation rounds unless it takes a [`Rounding`], and none
//! wraps or saturates. A result outside the range is `None`.

use crate::numeric::ArithmeticOperator;
use std::cmp::Ordering;

/// The largest scale, and the most significant digits, a `Decimal` holds.
pub(crate) const MAX_SCALE: u32 = 38;

/// The largest coefficient magnitude: 38 nines.
const MAX_MAGNITUDE: u128 = 10u128.pow(MAX_SCALE) - 1;

/// `magnitude × 10^-scale`, negative when `negative`.
///
/// Invariant: `magnitude <= MAX_MAGNITUDE`, `scale <= MAX_SCALE`, and zero is
/// never negative, so equal numbers at one scale have one representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Decimal {
    negative: bool,
    magnitude: u128,
    scale: u32,
}

/// How [`Decimal::divide`] rounds a quotient that its scale cannot hold. The
/// discriminants are the codes `std/decimal.rounding_code` passes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Rounding {
    TowardZero,
    AwayFromZero,
    Floor,
    Ceiling,
    HalfTowardZero,
    HalfAwayFromZero,
    HalfEven,
}

impl Rounding {
    pub(crate) const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::TowardZero,
            1 => Self::AwayFromZero,
            2 => Self::Floor,
            3 => Self::Ceiling,
            4 => Self::HalfTowardZero,
            5 => Self::HalfAwayFromZero,
            6 => Self::HalfEven,
            _ => return None,
        })
    }
}

/// Where a division's remainder falls against half the divisor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
enum Remainder {
    Zero,
    BelowHalf,
    Half,
    AboveHalf,
}

impl Decimal {
    /// `None` when the magnitude or scale is out of range.
    pub(crate) fn new(negative: bool, magnitude: u128, scale: u32) -> Option<Self> {
        (magnitude <= MAX_MAGNITUDE && scale <= MAX_SCALE).then_some(Self {
            negative: negative && magnitude != 0,
            magnitude,
            scale,
        })
    }

    /// `Decimal.of`: every `Int` coefficient fits.
    ///
    /// Requires: `scale <= MAX_SCALE`.
    pub(crate) fn of(coefficient: i64, scale: u32) -> Self {
        Self::new(
            coefficient < 0,
            u128::from(coefficient.unsigned_abs()),
            scale,
        )
        .expect("an Int coefficient has at most 19 digits")
    }

    /// The parts a constant row stores: the sign, the magnitude, the scale.
    pub(crate) const fn parts(self) -> (bool, u128, u32) {
        (self.negative, self.magnitude, self.scale)
    }

    pub(crate) const fn negate(self) -> Self {
        Self {
            negative: !self.negative && self.magnitude != 0,
            ..self
        }
    }

    pub(crate) fn add(self, other: Self) -> Option<Self> {
        let scale = self.scale.max(other.scale);
        let left = self
            .magnitude
            .checked_mul(power_of_ten(scale - self.scale)?)?;
        let right = other
            .magnitude
            .checked_mul(power_of_ten(scale - other.scale)?)?;
        // Either aligned magnitude may exceed the range while the sum does not;
        // past `u128` it cannot, because the other is at most 38 digits.
        if self.negative == other.negative {
            Self::new(self.negative, left.checked_add(right)?, scale)
        } else if left >= right {
            Self::new(self.negative, left - right, scale)
        } else {
            Self::new(other.negative, right - left, scale)
        }
    }

    pub(crate) fn subtract(self, other: Self) -> Option<Self> {
        self.add(other.negate())
    }

    pub(crate) fn multiply(self, other: Self) -> Option<Self> {
        Self::new(
            self.negative != other.negative,
            self.magnitude.checked_mul(other.magnitude)?,
            self.scale + other.scale,
        )
    }

    /// `self / divisor` with `scale` fraction digits, rounded by `rounding`, or
    /// `None` when `divisor` is zero or the rounded quotient is out of range.
    ///
    /// Requires: `scale <= MAX_SCALE`.
    pub(crate) fn divide(self, divisor: Self, scale: u32, rounding: Rounding) -> Option<Self> {
        if divisor.magnitude == 0 {
            return None;
        }
        // quotient = self.magnitude × 10^shift / divisor.magnitude
        let shift = i64::from(scale) + i64::from(divisor.scale) - i64::from(self.scale);
        let (quotient, remainder) = if shift >= 0 {
            long_division(self.magnitude, divisor.magnitude, shift as u32)?
        } else {
            match power_of_ten(shift.unsigned_abs() as u32)
                .and_then(|power| divisor.magnitude.checked_mul(power))
            {
                Some(denominator) => (
                    self.magnitude / denominator,
                    remainder_against(self.magnitude % denominator, denominator),
                ),
                // The divisor exceeds `u128`, so it is more than twice the
                // dividend, which has at most 38 digits.
                None if self.magnitude == 0 => (0, Remainder::Zero),
                None => (0, Remainder::BelowHalf),
            }
        };
        let negative = self.negative != divisor.negative;
        let inexact = remainder != Remainder::Zero;
        let away = match rounding {
            Rounding::TowardZero => false,
            Rounding::AwayFromZero => inexact,
            Rounding::Floor => negative && inexact,
            Rounding::Ceiling => !negative && inexact,
            Rounding::HalfTowardZero => remainder == Remainder::AboveHalf,
            Rounding::HalfAwayFromZero => remainder >= Remainder::Half,
            Rounding::HalfEven => {
                remainder == Remainder::AboveHalf
                    || (remainder == Remainder::Half && quotient % 2 == 1)
            }
        };
        Self::new(negative, quotient + u128::from(away), scale)
    }

    /// Numeric order: `1.50` and `1.5` are equal.
    pub(crate) fn numeric_cmp(self, other: Self) -> Ordering {
        let sign = |value: Self| match (value.magnitude, value.negative) {
            (0, _) => 0,
            (_, true) => -1,
            (_, false) => 1,
        };
        match sign(self).cmp(&sign(other)) {
            Ordering::Equal if self.negative => other.magnitude_cmp(self),
            Ordering::Equal => self.magnitude_cmp(other),
            unequal => unequal,
        }
    }

    /// Orders magnitudes, aligning the one with the smaller scale.
    fn magnitude_cmp(self, other: Self) -> Ordering {
        let aligned = |value: Self, scale: u32| {
            power_of_ten(scale - value.scale).and_then(|power| value.magnitude.checked_mul(power))
        };
        if self.scale >= other.scale {
            // Past `u128` the aligned side exceeds any 38-digit magnitude.
            aligned(other, self.scale).map_or(Ordering::Less, |right| self.magnitude.cmp(&right))
        } else {
            aligned(self, other.scale).map_or(Ordering::Greater, |left| left.cmp(&other.magnitude))
        }
    }

    /// `Decimal.to_text`: exactly `scale` fraction digits, as in `1.50` and `-3`.
    pub(crate) fn to_text(self) -> String {
        let digits = format!(
            "{:0>width$}",
            self.magnitude,
            width = self.scale as usize + 1
        );
        let (whole, fraction) = digits.split_at(digits.len() - self.scale as usize);
        let sign = if self.negative { "-" } else { "" };
        if fraction.is_empty() {
            format!("{sign}{whole}")
        } else {
            format!("{sign}{whole}.{fraction}")
        }
    }

    /// The debug form: the text with trailing fraction zeros removed, so equal
    /// values render the same (`1.50` and `1.5` are both `1.5`).
    pub(crate) fn normalized_text(self) -> String {
        let mut value = self;
        while value.scale > 0 && value.magnitude.is_multiple_of(10) {
            value.magnitude /= 10;
            value.scale -= 1;
        }
        value.to_text()
    }

    /// Reads `-?digits(.digits)?`; leading zeros are allowed. `None` for any
    /// other text, and for more than 38 significant or fraction digits.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (negative, unsigned) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (whole, fraction) = match unsigned.split_once('.') {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (unsigned, None),
        };
        let fraction = fraction.unwrap_or("");
        let digits_only = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
        if whole.is_empty()
            || !digits_only(whole)
            || !digits_only(fraction)
            || (unsigned.contains('.') && fraction.is_empty())
        {
            return None;
        }
        let mut magnitude = 0u128;
        for byte in whole.bytes().chain(fraction.bytes()) {
            // Checked before multiplying: 38 nines times 10 leaves `u128`.
            let digit = u128::from(byte - b'0');
            if magnitude > (MAX_MAGNITUDE - digit) / 10 {
                return None;
            }
            magnitude = magnitude * 10 + digit;
        }
        Self::new(negative, magnitude, u32::try_from(fraction.len()).ok()?)
    }

    /// The nearest `Float`, ties to even: the correctly rounded reading of the
    /// exact text, which every `u128` magnitude below `10^38` keeps finite.
    pub(crate) fn to_float(self) -> f64 {
        self.to_text()
            .parse()
            .expect("decimal text is a valid float literal")
    }
}

/// Applies a `Decimal` operator.
///
/// Ensures: `Err` holds the defect message, which names the operator and both
/// operands in `Decimal.to_text` form. Typing rejects `/` and `%` (`T4306`).
pub(crate) fn decimal_binary(
    operator: ArithmeticOperator,
    left: Decimal,
    right: Decimal,
) -> Result<Decimal, String> {
    let symbol = operator.symbol();
    let result = match operator {
        ArithmeticOperator::Add => left.add(right),
        ArithmeticOperator::Subtract => left.subtract(right),
        ArithmeticOperator::Multiply => left.multiply(right),
        ArithmeticOperator::Divide | ArithmeticOperator::Remainder => {
            return Err(format!("`{symbol}` is not a `Decimal` operator"));
        }
    };
    result.ok_or_else(|| {
        format!(
            "decimal overflow in `{symbol}`: {} {symbol} {} needs more than 38 digits or scale 38",
            left.to_text(),
            right.to_text()
        )
    })
}

fn power_of_ten(exponent: u32) -> Option<u128> {
    10u128.checked_pow(exponent)
}

/// `numerator × 10^shift / denominator` and where its remainder falls, or
/// `None` once the quotient leaves the range; rounding never shrinks it.
///
/// Requires: `0 < denominator <= MAX_MAGNITUDE`, so the remainder doubled
/// stays inside `u128`.
fn long_division(numerator: u128, denominator: u128, shift: u32) -> Option<(u128, Remainder)> {
    let mut quotient = numerator / denominator;
    let mut remainder = numerator % denominator;
    for _ in 0..shift {
        // remainder × 10 = digit × denominator + next, without leaving u128.
        let (mut digit, mut next) = (0u128, 0u128);
        for _ in 0..10 {
            next += remainder;
            if next >= denominator {
                next -= denominator;
                digit += 1;
            }
        }
        quotient = quotient.checked_mul(10)?.checked_add(digit)?;
        remainder = next;
    }
    (quotient <= MAX_MAGNITUDE).then_some((quotient, remainder_against(remainder, denominator)))
}

/// Requires: `remainder < denominator`.
fn remainder_against(remainder: u128, denominator: u128) -> Remainder {
    if remainder == 0 {
        return Remainder::Zero;
    }
    match remainder.cmp(&(denominator - remainder)) {
        Ordering::Less => Remainder::BelowHalf,
        Ordering::Equal => Remainder::Half,
        Ordering::Greater => Remainder::AboveHalf,
    }
}

#[cfg(test)]
mod tests {
    use super::{Decimal, Rounding};
    use std::cmp::Ordering;

    fn decimal(text: &str) -> Decimal {
        Decimal::parse(text).unwrap_or_else(|| panic!("{text} is a decimal"))
    }

    fn quotient(dividend: &str, divisor: &str, scale: u32, rounding: Rounding) -> Option<String> {
        decimal(dividend)
            .divide(decimal(divisor), scale, rounding)
            .map(Decimal::to_text)
    }

    // Each mode at a tie, off the tie on both sides, and for both signs; the
    // expected digits follow each mode's definition, not the algorithm.
    #[test]
    fn division_rounds_by_each_mode() {
        let cases = [
            // 2.5 is a tie, 2.6 and 2.4 are not; then their negatives.
            ("5", "2", ["2", "3", "2", "3", "2", "3", "2"]),
            ("7", "2", ["3", "4", "3", "4", "3", "4", "4"]),
            ("13", "5", ["2", "3", "2", "3", "3", "3", "3"]),
            ("12", "5", ["2", "3", "2", "3", "2", "2", "2"]),
            ("-5", "2", ["-2", "-3", "-3", "-2", "-2", "-3", "-2"]),
            ("-7", "2", ["-3", "-4", "-4", "-3", "-3", "-4", "-4"]),
            ("-13", "5", ["-2", "-3", "-3", "-2", "-3", "-3", "-3"]),
            ("-12", "5", ["-2", "-3", "-3", "-2", "-2", "-2", "-2"]),
            ("4", "2", ["2", "2", "2", "2", "2", "2", "2"]),
            ("-4", "2", ["-2", "-2", "-2", "-2", "-2", "-2", "-2"]),
        ];
        let modes = [
            Rounding::TowardZero,
            Rounding::AwayFromZero,
            Rounding::Floor,
            Rounding::Ceiling,
            Rounding::HalfTowardZero,
            Rounding::HalfAwayFromZero,
            Rounding::HalfEven,
        ];
        for (dividend, divisor, expected) in cases {
            for (mode, expected) in modes.iter().zip(expected) {
                assert_eq!(
                    quotient(dividend, divisor, 0, *mode).as_deref(),
                    Some(expected),
                    "{dividend} / {divisor} by {mode:?}"
                );
            }
        }
    }

    #[test]
    fn division_aligns_scales_and_reports_zero_and_overflow() {
        assert_eq!(
            quotient("1", "3", 4, Rounding::HalfEven).as_deref(),
            Some("0.3333")
        );
        assert_eq!(
            quotient("2", "3", 2, Rounding::HalfEven).as_deref(),
            Some("0.67")
        );
        assert_eq!(
            quotient("59.97", "4", 2, Rounding::HalfEven).as_deref(),
            Some("14.99")
        );
        assert_eq!(
            quotient("0.125", "1", 2, Rounding::HalfEven).as_deref(),
            Some("0.12")
        );
        assert_eq!(
            quotient("0.135", "1", 2, Rounding::HalfEven).as_deref(),
            Some("0.14")
        );
        // The divisor's scale raises the quotient: 1 / 0.001 = 1000.
        assert_eq!(
            quotient("1", "0.001", 0, Rounding::TowardZero).as_deref(),
            Some("1000")
        );
        // A divisor far above the dividend: the quotient is 0 or 1 ulp.
        assert_eq!(
            quotient(
                "0.00000000000000000000000000000000000001",
                "99999999999999999999999999999999999999",
                0,
                Rounding::Ceiling
            )
            .as_deref(),
            Some("1")
        );
        assert_eq!(
            quotient(
                "0.00000000000000000000000000000000000001",
                "99999999999999999999999999999999999999",
                0,
                Rounding::HalfEven
            )
            .as_deref(),
            Some("0")
        );
        assert_eq!(quotient("1", "0", 2, Rounding::HalfEven), None);
        assert_eq!(
            quotient(
                "99999999999999999999999999999999999999",
                "0.1",
                0,
                Rounding::HalfEven
            ),
            None
        );
        // At the edge: 38 nines / 1 fits; rounding it up does not.
        assert_eq!(
            quotient(
                "99999999999999999999999999999999999999",
                "1",
                0,
                Rounding::Ceiling
            )
            .as_deref(),
            Some("99999999999999999999999999999999999999")
        );
        assert_eq!(
            quotient(
                "99999999999999999999999999999999999999",
                "10",
                0,
                Rounding::Ceiling
            )
            .as_deref(),
            Some("10000000000000000000000000000000000000")
        );
        assert_eq!(
            quotient(
                "9999999999999999999999999999999999999.9",
                "1",
                0,
                Rounding::Ceiling
            )
            .as_deref(),
            Some("10000000000000000000000000000000000000")
        );
    }

    #[test]
    fn arithmetic_keeps_scales_and_reports_overflow() {
        let text = |value: Option<Decimal>| value.map(Decimal::to_text);
        assert_eq!(
            text(decimal("1.5").add(decimal("0.25"))).as_deref(),
            Some("1.75")
        );
        assert_eq!(
            text(decimal("1.50").subtract(decimal("1.5"))).as_deref(),
            Some("0.00")
        );
        assert_eq!(
            text(decimal("-0.5").add(decimal("0.25"))).as_deref(),
            Some("-0.25")
        );
        assert_eq!(
            text(decimal("19.99").multiply(decimal("3"))).as_deref(),
            Some("59.97")
        );
        assert_eq!(
            text(decimal("-1.5").multiply(decimal("0"))).as_deref(),
            Some("0.0")
        );
        let nines = decimal("99999999999999999999999999999999999999");
        assert_eq!(nines.add(decimal("1")), None);
        assert_eq!(nines.multiply(decimal("10")), None);
        assert_eq!(
            nines.add(nines.negate()).map(Decimal::to_text).as_deref(),
            Some("0")
        );
        // A scale-0 operand aligned past the range still sums into it.
        assert_eq!(
            text(
                decimal("18000000000000000000000000000000000000")
                    .add(decimal("-9000000000000000000000000000000000000.0"))
            )
            .as_deref(),
            Some("9000000000000000000000000000000000000.0")
        );
        let tiny = decimal("0.1");
        let deep = decimal("0.0000000000000000000000000000000000001");
        assert_eq!(
            text(tiny.multiply(deep)).as_deref(),
            Some("0.00000000000000000000000000000000000001")
        );
        assert_eq!(deep.multiply(deep), None);
    }

    #[test]
    fn order_is_numeric_across_scales_and_signs() {
        let cmp = |left: &str, right: &str| decimal(left).numeric_cmp(decimal(right));
        assert_eq!(cmp("1.50", "1.5"), Ordering::Equal);
        assert_eq!(cmp("-0.00", "0"), Ordering::Equal);
        assert_eq!(cmp("-2", "-1.5"), Ordering::Less);
        assert_eq!(cmp("1.49", "1.5"), Ordering::Less);
        assert_eq!(cmp("-1", "0.1"), Ordering::Less);
        assert_eq!(
            cmp(
                "99999999999999999999999999999999999999",
                "0.00000000000000000000000000000000000001"
            ),
            Ordering::Greater
        );
        assert_eq!(
            cmp(
                "0.00000000000000000000000000000000000001",
                "99999999999999999999999999999999999999"
            ),
            Ordering::Less
        );
    }

    #[test]
    fn text_keeps_the_scale_and_normalized_text_drops_trailing_zeros() {
        assert_eq!(Decimal::of(150, 2).to_text(), "1.50");
        assert_eq!(Decimal::of(150, 2).normalized_text(), "1.5");
        assert_eq!(Decimal::of(-5, 3).to_text(), "-0.005");
        assert_eq!(Decimal::of(100, 0).normalized_text(), "100");
        assert_eq!(Decimal::of(0, 3).normalized_text(), "0");
        assert_eq!(Decimal::of(i64::MIN, 0).to_text(), "-9223372036854775808");
    }

    #[test]
    fn parse_reads_exactly_the_decimal_grammar() {
        for (text, expected) in [
            ("0", Some("0")),
            ("-0.0", Some("0.0")),
            ("007.50", Some("7.50")),
            ("-19.99", Some("-19.99")),
            (
                "99999999999999999999999999999999999999",
                Some("99999999999999999999999999999999999999"),
            ),
            (
                "0.00000000000000000000000000000000000001",
                Some("0.00000000000000000000000000000000000001"),
            ),
            ("100000000000000000000000000000000000000", None),
            // Past 39 digits the accumulation would wrap `u128` (2^128 and up).
            ("999999999999999999999999999999999999999", None),
            ("340282366920938463463374607431768211456", None),
            ("340282366920938463463374607431768211457", None),
            ("3402823669209384634633746074317682114560", None),
            ("0.000000000000000000000000000000000000001", None),
            ("", None),
            ("-", None),
            ("+1", None),
            ("1.", None),
            (".5", None),
            ("1.2.3", None),
            ("1e2", None),
            ("1_000", None),
            (" 1", None),
            ("--1", None),
        ] {
            assert_eq!(
                Decimal::parse(text).map(Decimal::to_text).as_deref(),
                expected,
                "{text:?}"
            );
        }
    }

    #[test]
    fn to_float_rounds_to_nearest() {
        assert_eq!(decimal("0.1").to_float(), 0.1);
        assert_eq!(decimal("-19.99").to_float(), -19.99);
        // 2^53 + 1 is a tie between 2^53 and 2^53 + 2; ties go to even.
        assert_eq!(
            decimal("9007199254740993").to_float(),
            9_007_199_254_740_992.0
        );
    }
}
