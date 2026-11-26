//! The order of two JSON numbers by the exact decimal they denote, and the
//! canonical text that renders equal numbers alike, as in the interpreter's
//! `json.rs` (CHI-305 step 5, phase 4, third PR). A
//! written exponent may have more digits than any machine integer holds, so
//! exponents are decimal integers of any size and are never clamped.

use core::cmp::Ordering;

use alloc::vec::Vec;

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// A signed decimal integer of any size: `digits` are 0-9, most significant
/// first, with no leading zeros. Zero has no digits and is never negative.
struct Exponent {
    negative: bool,
    digits: Vec<u8>,
}

/// `length` bytes of room, or the run ends.
fn room(length: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(length).is_err() {
        // SAFETY: a function of runtime.c that does not return.
        unsafe { chimi_runtime_exhausted_memory() }
    }
    bytes
}

impl Exponent {
    const ZERO: Self = Self { negative: false, digits: Vec::new() };

    /// From digit values, most significant first, dropping leading zeros.
    fn new(negative: bool, digits: &[u8]) -> Self {
        let first = digits.iter().position(|&digit| digit != 0).unwrap_or(digits.len());
        let digits = &digits[first..];
        let mut owned = room(digits.len());
        owned.extend_from_slice(digits);
        Self { negative: negative && !owned.is_empty(), digits: owned }
    }

    fn of(value: i64) -> Self {
        let mut magnitude = value.unsigned_abs();
        let mut digits = [0u8; 20];
        let mut start = digits.len();
        while magnitude != 0 {
            start -= 1;
            digits[start] = (magnitude % 10) as u8;
            magnitude /= 10;
        }
        Self::new(value < 0, &digits[start..])
    }

    fn magnitude_order(&self, other: &Self) -> Ordering {
        self.digits.len().cmp(&other.digits.len()).then_with(|| self.digits.cmp(&other.digits))
    }

    fn order(&self, other: &Self) -> Ordering {
        let rank = |exponent: &Self| if exponent.negative { -1 } else { i32::from(!exponent.digits.is_empty()) };
        rank(self).cmp(&rank(other)).then_with(|| {
            let order = self.magnitude_order(other);
            if self.negative { order.reverse() } else { order }
        })
    }

    fn sum(&self, other: &Self) -> Self {
        let order = self.magnitude_order(other);
        let subtract = self.negative != other.negative;
        if subtract && order == Ordering::Equal {
            return Self::ZERO;
        }
        let (larger, smaller) = if order.is_ge() { (self, other) } else { (other, self) };
        let length = larger.digits.len() + 1;
        let mut result = room(length);
        result.resize(length, 0);
        let mut carry = 0i32;
        for place in 0..length {
            let digit_at = |digits: &[u8]| {
                if place < digits.len() { i32::from(digits[digits.len() - 1 - place]) } else { 0 }
            };
            let (big, small) = (digit_at(&larger.digits), digit_at(&smaller.digits));
            let digit = if subtract { big - small - carry } else { big + small + carry };
            carry = i32::from(if subtract { digit < 0 } else { digit > 9 });
            result[length - 1 - place] = (if subtract { digit + 10 * carry } else { digit - 10 * carry }) as u8;
        }
        Self::new(larger.negative, &result)
    }
}

/// A JSON number as sign x 0.digits x 10^exponent with no leading or
/// trailing zeros in `digits`; zero has sign 0.
struct Canonical {
    sign: i32,
    digits: Vec<u8>,
    exponent: Exponent,
}

/// `text` is a JSON number as the parser accepted it.
fn canonical(text: &[u8]) -> Canonical {
    let negative = text.first() == Some(&b'-');
    let text = if negative { &text[1..] } else { text };
    let mantissa = text.iter().position(|&byte| byte == b'e' || byte == b'E').unwrap_or(text.len());
    let mut written = Exponent::ZERO;
    if mantissa < text.len() {
        let mut digits = &text[mantissa + 1..];
        let negative_exponent = digits.first() == Some(&b'-');
        if matches!(digits.first(), Some(b'-' | b'+')) {
            digits = &digits[1..];
        }
        let mut values = room(digits.len());
        values.extend(digits.iter().map(|digit| digit.wrapping_sub(b'0')));
        written = Exponent::new(negative_exponent, &values);
    }
    let point = text[..mantissa].iter().position(|&byte| byte == b'.').unwrap_or(mantissa);
    let mut digits = room(mantissa);
    digits.extend(text[..mantissa].iter().copied().filter(|&byte| byte != b'.'));
    let leading = digits.iter().position(|&digit| digit != b'0').unwrap_or(digits.len());
    let length = digits.iter().rposition(|&digit| digit != b'0').map_or(leading, |last| last + 1);
    if length == leading {
        return Canonical { sign: 0, digits: Vec::new(), exponent: written };
    }
    digits.truncate(length);
    digits.drain(..leading);
    let offset = Exponent::of(point as i64 - leading as i64);
    Canonical { sign: if negative { -1 } else { 1 }, digits, exponent: written.sum(&offset) }
}

fn order_of(order: Ordering) -> i32 {
    order as i32
}

/// -1, 0 or 1 as the number `left` denotes is less than, equal to or
/// greater than the one `right` denotes.
pub fn compare(left: &[u8], right: &[u8]) -> i32 {
    let (left, right) = (canonical(left), canonical(right));
    let mut order = left.sign.cmp(&right.sign);
    if order == Ordering::Equal && left.sign != 0 {
        order = left.exponent.order(&right.exponent).then_with(|| left.digits.cmp(&right.digits));
        if left.sign < 0 {
            order = order.reverse();
        }
    }
    order_of(order)
}

/// json.rs `canonical_number_text`: the exact value, so equal numbers render
/// alike. Positional when the decimal exponent of the first significant
/// digit is from -6 to 20, otherwise d.ddde<exponent>, as in 1e21 and 1e400.
pub fn canonical_text(text: &[u8]) -> Vec<u8> {
    let number = canonical(text);
    let mut out = room(number.digits.len() + 32);
    if number.sign == 0 {
        out.push(b'0');
        return out;
    }
    if number.sign < 0 {
        out.push(b'-');
    }
    let digits = &number.digits[..];
    // 0.digits x 10^exponent: `exponent` digits come before the point.
    let positional = number.exponent.order(&Exponent::of(-5)).is_ge() && number.exponent.order(&Exponent::of(21)).is_le();
    let extend = |out: &mut Vec<u8>, bytes: &[u8]| {
        if out.try_reserve(bytes.len()).is_err() {
            // SAFETY: a function of runtime.c that does not return.
            unsafe { chimi_runtime_exhausted_memory() }
        }
        out.extend_from_slice(bytes);
    };
    if positional {
        let magnitude = number.exponent.digits.iter().fold(0i64, |sum, &digit| sum * 10 + i64::from(digit));
        let before = if number.exponent.negative { -magnitude } else { magnitude };
        let length = digits.len() as i64;
        if before >= length {
            extend(&mut out, digits);
            for _ in length..before {
                extend(&mut out, b"0");
            }
        } else if before > 0 {
            extend(&mut out, &digits[..before as usize]);
            extend(&mut out, b".");
            extend(&mut out, &digits[before as usize..]);
        } else {
            extend(&mut out, b"0.");
            for _ in 0..-before {
                extend(&mut out, b"0");
            }
            extend(&mut out, digits);
        }
    } else {
        extend(&mut out, &digits[..1]);
        if digits.len() > 1 {
            extend(&mut out, b".");
            extend(&mut out, &digits[1..]);
        }
        let scientific = number.exponent.sum(&Exponent::of(-1));
        extend(&mut out, b"e");
        if scientific.negative {
            extend(&mut out, b"-");
        }
        for digit in &scientific.digits {
            extend(&mut out, &[b'0' + digit]);
        }
    }
    out
}
