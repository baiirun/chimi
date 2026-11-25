//! The `Duration` model, shared by the type checker's constant checks, the
//! reference interpreter and the native runtime: a signed 64-bit count of
//! nanoseconds (`time-and-outcomes.md`, "Duration"). `runtime.c` reaches its
//! text form and parser through [`crate::duration_abi`]. The arithmetic and
//! its defect messages are each backend's own (the compiler's `duration.rs`
//! and `runtime.c`), which `tests/language.rs` compares on a generated set.

use crate::text_buffer::TextBuffer;
use alloc::string::String;
use alloc::vec::Vec;

/// Room for the longest text, `-2562047h47m16.854775808s` (25 bytes); with a
/// NUL it fits the 32-byte buffers `runtime.c` passes.
pub const TEXT_BYTES: usize = 31;

/// [`text`]'s result.
pub type DurationText = TextBuffer<TEXT_BYTES>;

/// A unit a `Duration` is built from or written in, from the largest down.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unit {
    Hours,
    Minutes,
    Seconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
}

impl Unit {
    /// Largest first: the order the text form requires.
    pub const DESCENDING: [Self; 6] = [
        Self::Hours,
        Self::Minutes,
        Self::Seconds,
        Self::Milliseconds,
        Self::Microseconds,
        Self::Nanoseconds,
    ];

    pub const fn nanoseconds(self) -> i64 {
        match self {
            Self::Hours => 3_600_000_000_000,
            Self::Minutes => 60_000_000_000,
            Self::Seconds => 1_000_000_000,
            Self::Milliseconds => 1_000_000,
            Self::Microseconds => 1_000,
            Self::Nanoseconds => 1,
        }
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::Hours => "h",
            Self::Minutes => "m",
            Self::Seconds => "s",
            Self::Milliseconds => "ms",
            Self::Microseconds => "us",
            Self::Nanoseconds => "ns",
        }
    }

    /// The `std/duration` constructor that takes a count of this unit.
    pub const fn constructor(self) -> &'static str {
        match self {
            Self::Hours => "hours",
            Self::Minutes => "minutes",
            Self::Seconds => "seconds",
            Self::Milliseconds => "milliseconds",
            Self::Microseconds => "microseconds",
            Self::Nanoseconds => "nanoseconds",
        }
    }
}

/// `Duration.to_<unit>`: whole units, truncated toward zero.
pub const fn whole_units(duration: i64, unit: Unit) -> i64 {
    duration / unit.nanoseconds()
}

/// The canonical text form: `0s`, `-1.5ms`, `250us`, `1h0m30s`, `2m3.25s`.
pub fn text(duration: i64) -> DurationText {
    let mut text = TextBuffer::new();
    if duration == 0 {
        text.push_str("0s");
        return text;
    }
    if duration < 0 {
        text.push(b'-');
    }
    // The magnitude of the minimum value does not fit `i64`.
    let magnitude = duration.unsigned_abs();
    let unit = |unit: Unit| unit.nanoseconds().unsigned_abs();
    if magnitude < unit(Unit::Seconds) {
        let small = if magnitude >= unit(Unit::Milliseconds) {
            Unit::Milliseconds
        } else if magnitude >= unit(Unit::Microseconds) {
            Unit::Microseconds
        } else {
            Unit::Nanoseconds
        };
        push_decimal(&mut text, magnitude, unit(small));
        text.push_str(small.suffix());
        return text;
    }
    let parts = [
        (magnitude / unit(Unit::Hours), Unit::Hours),
        (
            magnitude % unit(Unit::Hours) / unit(Unit::Minutes),
            Unit::Minutes,
        ),
        // In nanoseconds, so the fraction stays exact.
        (magnitude % unit(Unit::Minutes), Unit::Seconds),
    ];
    // A magnitude of at least one second has a nonzero part.
    let first = parts.iter().position(|(count, _)| *count != 0).unwrap_or(0);
    let last = parts.iter().rposition(|(count, _)| *count != 0).unwrap_or(0);
    for (count, part_unit) in &parts[first..=last] {
        if *part_unit == Unit::Seconds {
            push_decimal(&mut text, *count, unit(Unit::Seconds));
        } else {
            text.push_digits(u128::from(*count), 1);
        }
        text.push_str(part_unit.suffix());
    }
    text
}

pub fn to_text(duration: i64) -> String {
    String::from(text(duration).as_str())
}

/// Writes `value / scale` as an exact decimal with no trailing fraction zeros.
fn push_decimal(text: &mut DurationText, value: u64, scale: u64) {
    text.push_digits(u128::from(value / scale), 1);
    let fraction = value % scale;
    if fraction != 0 {
        text.push(b'.');
        let point = text.len();
        text.push_digits(u128::from(fraction), scale.ilog10() as usize);
        text.trim_end(b'0', point);
    }
}

/// Why `Duration.parse` rejected a text: the variants of `std/duration.ParseError`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseFailure {
    /// The empty text.
    Empty,
    /// Not the grammar, or a fraction that is not a whole number of nanoseconds.
    Invalid,
    /// Grammatical and exact, but outside the 64-bit range.
    OutOfRange,
}

/// One `<decimal><unit>` part, as an exact count of nanoseconds.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub unit: Unit,
    /// Fits `i64` when negated together with the other parts (checked by `parse`).
    pub nanoseconds: i128,
}

/// A text [`parse`] reads, part by part, before the parts are summed.
pub struct Parsed {
    pub negative: bool,
    pub parts: Vec<Part>,
}

/// `Duration.parse`: an optional `-`, then one or more `<decimal><unit>` parts with
/// units in strictly descending order, or the literal `0`. A decimal is digits with an
/// optional `.digits` fraction.
///
/// Ensures: `parse(&to_text(d)) == Ok(d)` for every `d`; never rounds (`1.5ns` is
/// `Invalid`); a grammatical, exact text outside the range is `OutOfRange`.
pub fn parse(text: &str) -> Result<i64, ParseFailure> {
    let parsed = parse_parts(text)?;
    let magnitude = parsed
        .parts
        .iter()
        .try_fold(0_i128, |total, part| total.checked_add(part.nanoseconds))
        .ok_or(ParseFailure::OutOfRange)?;
    let value = if parsed.negative {
        -magnitude
    } else {
        magnitude
    };
    i64::try_from(value).map_err(|_| ParseFailure::OutOfRange)
}

/// The digits of a part's integer or fraction above which no value is in range or exact.
/// An exact fraction has at most 13 significant digits: `h` is 2^13 * 5^11 * 9 ns.
const MAX_INTEGER_DIGITS: usize = 22;
const MAX_FRACTION_DIGITS: usize = 13;

/// The parts of a text in the grammar [`parse`] reads, unsummed.
pub fn parse_parts(text: &str) -> Result<Parsed, ParseFailure> {
    if text.is_empty() {
        return Err(ParseFailure::Empty);
    }
    if text == "0" {
        return Ok(Parsed {
            negative: false,
            parts: Vec::new(),
        });
    }
    let (negative, mut rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut parts = Vec::<Part>::new();
    let mut out_of_range = false;
    while !rest.is_empty() {
        let integer_length = rest.bytes().take_while(u8::is_ascii_digit).count();
        if integer_length == 0 {
            return Err(ParseFailure::Invalid);
        }
        let (integer, after) = rest.split_at(integer_length);
        let (fraction, after) = match after.strip_prefix('.') {
            Some(after) => {
                let length = after.bytes().take_while(u8::is_ascii_digit).count();
                if length == 0 {
                    return Err(ParseFailure::Invalid);
                }
                after.split_at(length)
            }
            None => ("", after),
        };
        let unit = Unit::DESCENDING
            .into_iter()
            // `ms` before `m`: the longest suffix wins.
            .filter(|unit| after.starts_with(unit.suffix()))
            .max_by_key(|unit| unit.suffix().len())
            .ok_or(ParseFailure::Invalid)?;
        rest = &after[unit.suffix().len()..];
        if parts
            .last()
            .is_some_and(|previous| previous.unit.nanoseconds() <= unit.nanoseconds())
        {
            return Err(ParseFailure::Invalid);
        }
        let scale = i128::from(unit.nanoseconds());
        let fraction = fraction.trim_end_matches('0');
        if fraction.len() > MAX_FRACTION_DIGITS {
            return Err(ParseFailure::Invalid);
        }
        let denominator = 10_i128.pow(fraction.len() as u32);
        let numerator = if fraction.is_empty() {
            0
        } else {
            digits_value(fraction) * scale
        };
        if numerator % denominator != 0 {
            return Err(ParseFailure::Invalid);
        }
        let integer = integer.trim_start_matches('0');
        let nanoseconds = if integer.len() > MAX_INTEGER_DIGITS {
            out_of_range = true;
            0
        } else {
            digits_value(integer) * scale + numerator / denominator
        };
        parts.push(Part { unit, nanoseconds });
    }
    if parts.is_empty() {
        return Err(ParseFailure::Invalid);
    }
    if out_of_range {
        return Err(ParseFailure::OutOfRange);
    }
    Ok(Parsed { negative, parts })
}

/// The value of at most 38 ASCII digits; 0 for none.
fn digits_value(digits: &str) -> i128 {
    digits
        .bytes()
        .fold(0, |value, digit| value * 10 + i128::from(digit - b'0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_text_follows_the_spec_examples() {
        let second = Unit::Seconds.nanoseconds();
        for (value, text) in [
            (0, "0s"),
            (1_500_000, "1.5ms"),
            (250_000, "250us"),
            (7, "7ns"),
            (-7, "-7ns"),
            (3_600 * second, "1h"),
            (5_400 * second, "1h30m"),
            (3_630 * second, "1h0m30s"),
            (123 * second + second / 4, "2m3.25s"),
            (second + 1, "1.000000001s"),
            (3_600 * second + 1, "1h0m0.000000001s"),
            (i64::MAX, "2562047h47m16.854775807s"),
            (i64::MIN, "-2562047h47m16.854775808s"),
        ] {
            assert_eq!(to_text(value), text, "{value}");
            assert_eq!(parse(text), Ok(value), "{text}");
        }
    }

    #[test]
    fn parse_rejects_what_the_grammar_rejects_and_never_rounds() {
        for text in [
            "-",
            "s",
            "1",
            "-0",
            "1.5ns",
            "1s1h",
            "1m1m",
            "1ms1m",
            "1.s",
            ".5s",
            "1 s",
            "+1s",
            "1x",
            "1h-1m",
            "1µs",
            "0.0000000005s",
            "1s ",
        ] {
            assert_eq!(parse(text), Err(ParseFailure::Invalid), "{text:?}");
        }
        for (text, value) in [
            ("0", 0),
            ("-0s", 0),
            ("1.0ns", 1),
            ("1.5h", 5_400_000_000_000),
            ("90s", 90_000_000_000),
            ("001s", 1_000_000_000),
            ("1.50s", 1_500_000_000),
            ("1h1ms", 3_600_001_000_000),
        ] {
            assert_eq!(parse(text), Ok(value), "{text:?}");
        }
        for text in [
            "2562047h47m16.854775808s",
            "-2562047h47m16.854775809s",
            "9223372036854775808ns",
            "99999999999999999999999999999h",
        ] {
            assert_eq!(parse(text), Err(ParseFailure::OutOfRange), "{text:?}");
        }
        assert_eq!(parse(""), Err(ParseFailure::Empty));
        // A grammar error anywhere outranks a range error.
        assert_eq!(
            parse("99999999999999999999999999999h1x"),
            Err(ParseFailure::Invalid)
        );
    }
}
