//! The canonical text of a finite `Float`, which both backends print: the
//! shortest digits that parse back to the same value, always with a `.` or an
//! exponent, and `-0.0` for negative zero. Positional notation is used for
//! decimal exponents from -5 to 15, scientific notation (`1e16`, `2.5e-7`)
//! otherwise.

use crate::TextBuffer;

/// The longest text: a sign, 17 digits, a point and `e-308`, or a sign,
/// `0.0000` and 17 digits.
pub const TEXT_BYTES: usize = 32;
pub type FloatText = TextBuffer<TEXT_BYTES>;

/// The text of `value`. Requires: `value` is finite.
pub fn text(value: f64) -> FloatText {
    debug_assert!(value.is_finite());
    let mut text = FloatText::new();
    if value.is_sign_negative() {
        text.push(b'-');
    }
    if value == 0.0 {
        text.push_str("0.0");
        return text;
    }
    let (digits, exponent) = shortest_digits(value.abs());
    let digits = digits.as_bytes();
    if (-5..16).contains(&exponent) {
        if exponent < 0 {
            text.push_str("0.");
            for _ in 0..(-exponent - 1) {
                text.push(b'0');
            }
            push_bytes(&mut text, digits);
        } else {
            let whole = exponent as usize + 1;
            if digits.len() <= whole {
                push_bytes(&mut text, digits);
                for _ in digits.len()..whole {
                    text.push(b'0');
                }
                text.push_str(".0");
            } else {
                push_bytes(&mut text, &digits[..whole]);
                text.push(b'.');
                push_bytes(&mut text, &digits[whole..]);
            }
        }
    } else {
        text.push(digits[0]);
        if digits.len() > 1 {
            text.push(b'.');
            push_bytes(&mut text, &digits[1..]);
        }
        text.push(b'e');
        if exponent < 0 {
            text.push(b'-');
        }
        text.push_digits(u128::from(exponent.unsigned_abs()), 1);
    }
    text
}

fn push_bytes(text: &mut FloatText, bytes: &[u8]) {
    for byte in bytes {
        text.push(*byte);
    }
}

/// The shortest round-trip significand digits of a positive finite
/// `magnitude` and its decimal exponent, from core's `{:e}` (as in
/// `1.2345e3`), which writes into a fixed buffer, not the heap.
fn shortest_digits(magnitude: f64) -> (TextBuffer<24>, i32) {
    let mut scientific = Scientific::default();
    // `{:e}` on a finite f64 writes at most 17 digits, a point and `e-324`.
    let _ = core::fmt::write(&mut scientific, format_args!("{magnitude:e}"));
    let written = &scientific.bytes[..scientific.length];
    let split = written.iter().position(|byte| *byte == b'e').unwrap_or(written.len());
    let mut digits = TextBuffer::new();
    for byte in &written[..split] {
        if *byte != b'.' {
            digits.push(*byte);
        }
    }
    let (negative, exponent_digits) = match written.get(split + 1..).unwrap_or_default() {
        [b'-', rest @ ..] => (true, rest),
        rest => (false, rest),
    };
    let magnitude_exponent = exponent_digits
        .iter()
        .fold(0i32, |value, digit| value * 10 + i32::from(digit - b'0'));
    let exponent = if negative { -magnitude_exponent } else { magnitude_exponent };
    (digits, exponent)
}

/// What `{:e}` writes, at most 32 ASCII bytes.
#[derive(Default)]
struct Scientific {
    bytes: [u8; 32],
    length: usize,
}

impl core::fmt::Write for Scientific {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let slot = self.bytes.get_mut(self.length..end).ok_or(core::fmt::Error)?;
        slot.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::text;

    #[test]
    fn floats_print_their_shortest_round_trip_text() {
        for (value, expected) in [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1.5e-5, "0.000015"),
            (1e-6, "1e-6"),
            (123_456_789_012_345.6, "123456789012345.6"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e16"),
            (2.5e-7, "2.5e-7"),
            (f64::MAX, "1.7976931348623157e308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (5e-324, "5e-324"),
            (-1.0 / 3.0, "-0.3333333333333333"),
        ] {
            assert_eq!(text(value).as_str(), expected, "{value:e}");
        }
    }
}
