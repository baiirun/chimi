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
/// `magnitude` and its decimal exponent, chosen as core's `{:e}` chooses
/// them: at the shortest length, the candidate closest to the value, and the
/// larger one on an exact tie. The C library's correctly rounded `%.*e`
/// gives the closest candidate at each length; at a power of two the
/// interval below the value is half as wide, so that candidate can miss it
/// while a neighbor fits, and the neighbors are tried too. This is the C
/// the fuzz oracle keeps (tests/fuzz/float-c-reference.c), and it leaves
/// core's float formatting, about 12 KiB, out of every program.
fn shortest_digits(magnitude: f64) -> (TextBuffer<24>, i32) {
    for precision in 0..=16 {
        let mut scientific = [0u8; 40];
        // SAFETY: the buffer holds 40 bytes, and `%.16e` of a finite double
        // needs at most 24 with its NUL.
        unsafe {
            libc::snprintf(scientific.as_mut_ptr().cast(), scientific.len(), c"%.*e".as_ptr(), precision, magnitude);
        }
        let split = scientific.iter().position(|&byte| byte == b'e').unwrap_or(0);
        let nearest = scientific[..split]
            .iter()
            .filter(|&&byte| byte != b'.')
            .fold(0u64, |value, &digit| value * 10 + u64::from(digit - b'0'));
        let scale = exponent_of(&scientific[split + 1..]) - precision;
        let smallest = 10u64.pow(precision as u32);
        let chosen = if round_trips(nearest, scale, magnitude) {
            if round_trips(nearest + 1, scale, magnitude) && is_tie(magnitude, nearest) { nearest + 1 } else { nearest }
        } else if round_trips(nearest + 1, scale, magnitude) {
            nearest + 1
        } else if nearest > smallest && round_trips(nearest - 1, scale, magnitude) {
            nearest - 1
        } else {
            continue;
        };
        let mut digits = TextBuffer::<24>::new();
        digits.push_digits(u128::from(chosen), 1);
        // A carry such as 99 + 1 adds a digit; the zero it leaves is dropped.
        let exponent = scale + digits.len() as i32 - 1;
        digits.trim_end(b'0', 1);
        return (digits, exponent);
    }
    // 17 significant digits always round-trip.
    let mut scientific = [0u8; 40];
    // SAFETY: as above.
    unsafe {
        libc::snprintf(scientific.as_mut_ptr().cast(), scientific.len(), c"%.16e".as_ptr(), magnitude);
    }
    let split = scientific.iter().position(|&byte| byte == b'e').unwrap_or(0);
    let mut digits = TextBuffer::<24>::new();
    for &byte in scientific[..split].iter().filter(|&&byte| byte != b'.') {
        digits.push(byte);
    }
    digits.trim_end(b'0', 1);
    (digits, exponent_of(&scientific[split + 1..]))
}

/// The exponent `%e` wrote after its `e`, up to the NUL.
fn exponent_of(written: &[u8]) -> i32 {
    let (negative, digits) = match written {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        rest => (false, rest),
    };
    let value = digits
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .fold(0i32, |value, &digit| value * 10 + i32::from(digit - b'0'));
    if negative { -value } else { value }
}

/// Whether `significand` x 10^`exponent` parses back to `magnitude`.
fn round_trips(significand: u64, exponent: i32, magnitude: f64) -> bool {
    let mut text = TextBuffer::<48>::new();
    text.push_digits(u128::from(significand), 1);
    text.push(b'e');
    if exponent < 0 {
        text.push(b'-');
    }
    text.push_digits(u128::from(exponent.unsigned_abs()), 1);
    text.push(0);
    // SAFETY: the text ends with the NUL just pushed.
    unsafe { libc::strtod(text.as_bytes().as_ptr().cast(), core::ptr::null_mut()) == magnitude }
}

/// Whether `magnitude` is exactly halfway between `nearest` and the next
/// candidate: its exact digits are `nearest`'s, then a 5, then only zeros.
fn is_tie(magnitude: f64, nearest: u64) -> bool {
    let mut exact = [0u8; 800];
    // SAFETY: the buffer holds 800 bytes; a double's exact digits number at
    // most 767, written after one digit and a point.
    unsafe {
        libc::snprintf(exact.as_mut_ptr().cast(), exact.len(), c"%.770e".as_ptr(), magnitude);
    }
    let split = exact.iter().position(|&byte| byte == b'e').unwrap_or(0);
    let mut wanted = TextBuffer::<24>::new();
    wanted.push_digits(u128::from(nearest), 1);
    let mut actual = exact[..split].iter().copied().filter(|&byte| byte != b'.');
    wanted.as_bytes().iter().all(|&digit| actual.next() == Some(digit))
        && actual.next() == Some(b'5')
        && actual.all(|byte| byte == b'0')
}

#[cfg(test)]
mod tests {
    use super::{shortest_digits, text};

    /// core's `{:e}` digits and exponent, which `shortest_digits` matches.
    fn core_digits(magnitude: f64) -> (String, i32) {
        let written = format!("{magnitude:e}");
        let (significand, exponent) = written.split_once('e').unwrap();
        (significand.replace('.', ""), exponent.parse().unwrap())
    }

    #[test]
    fn shortest_digits_match_core_on_every_kind_of_double() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut values = vec![5e-324, f64::MIN_POSITIVE, f64::MAX, 1.0, 0.3, 2f64.powi(-1074), 9007199254740993.0];
        // Powers of two, where the interval below is half as wide, and
        // random bit patterns across the whole range.
        values.extend((-1074..1024).map(|power| 2f64.powi(power)));
        for _ in 0..200_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state & !(1 << 63));
            if value.is_finite() && value != 0.0 {
                values.push(value);
            }
        }
        for value in values {
            let (digits, exponent) = shortest_digits(value);
            assert_eq!((digits.as_str().to_owned(), exponent), core_digits(value), "{value:e}");
        }
    }

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
