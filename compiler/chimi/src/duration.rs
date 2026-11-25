//! The `Duration` model lives in `chimi-runtime`, which the native runtime
//! links too, so both backends print and parse durations with one
//! implementation. The arithmetic and its defect messages are the compiler's
//! own; `runtime.c` keeps matching ones, and `tests/language.rs` runs a
//! generated set through both.
//!
//! Invariant: no operation wraps or saturates. A result outside `i64` is an `Err` holding
//! the defect message.

pub(crate) use chimi_runtime::duration::{ParseFailure, Unit, parse, parse_parts, to_text, whole_units};

/// `Duration.<unit>(count)`.
pub(crate) fn construct(unit: Unit, count: i64) -> Result<i64, String> {
    count.checked_mul(unit.nanoseconds()).ok_or_else(|| {
        format!(
            "`Duration.{}({count})` overflows the Duration range",
            unit.constructor()
        )
    })
}

pub(crate) fn add(left: i64, right: i64) -> Result<i64, String> {
    left.checked_add(right)
        .ok_or_else(|| overflow("add", &format!("{} + {}", to_text(left), to_text(right))))
}

pub(crate) fn subtract(left: i64, right: i64) -> Result<i64, String> {
    left.checked_sub(right).ok_or_else(|| {
        overflow(
            "subtract",
            &format!("{} - {}", to_text(left), to_text(right)),
        )
    })
}

pub(crate) fn multiply(duration: i64, factor: i64) -> Result<i64, String> {
    duration
        .checked_mul(factor)
        .ok_or_else(|| overflow("multiply", &format!("{} * {factor}", to_text(duration))))
}

/// Truncates toward zero.
pub(crate) fn divide(duration: i64, divisor: i64) -> Result<i64, String> {
    if divisor == 0 {
        return Err(format!(
            "`Duration.divide` by zero: {} / 0",
            to_text(duration)
        ));
    }
    duration
        .checked_div(divisor)
        .ok_or_else(|| overflow("divide", &format!("{} / {divisor}", to_text(duration))))
}

pub(crate) fn negate(duration: i64) -> Result<i64, String> {
    duration
        .checked_neg()
        .ok_or_else(|| overflow("negate", &format!("-({})", to_text(duration))))
}

pub(crate) fn absolute(duration: i64) -> Result<i64, String> {
    duration
        .checked_abs()
        .ok_or_else(|| overflow("absolute", &format!("|{}|", to_text(duration))))
}

fn overflow(function: &str, operation: &str) -> String {
    format!("`Duration.{function}` overflows the Duration range: {operation}")
}

/// For a text `parse` accepts, the constructor expression with the same value:
/// `Duration.seconds(5)` for `5s`, `Duration.add(Duration.hours(1), Duration.minutes(30))`
/// for `1h30m`, `Duration.milliseconds(-1500)` for `-1.5s`, and `Duration.zero` for zero.
///
/// Ensures: each part becomes one constructor in its own unit, or in the largest smaller
/// unit that makes its count whole; zero parts are left out; a negative text negates each
/// count, so even the minimum value builds without overflow.
pub(crate) fn constructor_expression(text: &str) -> Option<String> {
    parse(text).ok()?;
    let parsed = parse_parts(text).ok()?;
    let calls = parsed
        .parts
        .iter()
        .filter(|part| part.nanoseconds != 0)
        .map(|part| {
            let unit = Unit::DESCENDING
                .into_iter()
                .skip_while(|unit| *unit != part.unit)
                .find(|unit| part.nanoseconds % i128::from(unit.nanoseconds()) == 0)
                .expect("every part is a whole number of nanoseconds");
            let count = part.nanoseconds / i128::from(unit.nanoseconds());
            let count = if parsed.negative { -count } else { count };
            format!("Duration.{}({count})", unit.constructor())
        });
    calls
        .reduce(|sum, call| format!("Duration.add({sum}, {call})"))
        .or_else(|| Some("Duration.zero".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_expressions_have_the_parsed_value() {
        for (text, expression) in [
            ("5s", "Duration.seconds(5)"),
            (
                "1h30m",
                "Duration.add(Duration.hours(1), Duration.minutes(30))",
            ),
            ("-1.5s", "Duration.milliseconds(-1500)"),
            ("1h0m", "Duration.hours(1)"),
            ("0", "Duration.zero"),
            ("0s", "Duration.zero"),
        ] {
            assert_eq!(constructor_expression(text).as_deref(), Some(expression));
        }
        assert_eq!(constructor_expression("5x"), None);
    }

    #[test]
    fn arithmetic_defects_instead_of_wrapping() {
        assert!(construct(Unit::Hours, 2_562_047).is_ok());
        assert!(construct(Unit::Hours, 2_562_048).is_err());
        assert!(construct(Unit::Hours, -2_562_048).is_err());
        assert_eq!(add(i64::MAX - 1, 1), Ok(i64::MAX));
        assert!(add(i64::MAX, 1).is_err());
        assert!(subtract(i64::MIN, 1).is_err());
        assert!(multiply(i64::MAX, 2).is_err());
        assert_eq!(divide(-7, 2), Ok(-3));
        assert!(divide(1, 0).is_err());
        assert!(divide(i64::MIN, -1).is_err());
        assert!(negate(i64::MIN).is_err());
        assert!(absolute(i64::MIN).is_err());
        assert_eq!(absolute(i64::MIN + 1), Ok(i64::MAX));
        assert_eq!(whole_units(-1_999_999, Unit::Milliseconds), -1);
    }
}
