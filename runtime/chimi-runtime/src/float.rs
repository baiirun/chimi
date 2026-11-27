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
/// larger one on an exact tie. This is the exact digit generation core falls
/// back to (Steele and White's, as Burger and Dybvig state it) over a fixed
/// big integer; it leaves core's float formatting, about 12 KiB, out of
/// every program.
///
/// The value is `r / s`, and the doubles next to it lie `m_minus` below and
/// `m_plus` above, so any decimal closer than halfway to them reads back as
/// this value; the interval includes its ends when the significand is even,
/// as round-half-even parsing does.
fn shortest_digits(magnitude: f64) -> (TextBuffer<24>, i32) {
    let bits = magnitude.to_bits();
    let fraction = bits & ((1 << 52) - 1);
    let biased = (bits >> 52) as i32;
    let (significand, exponent) = if biased == 0 { (fraction, -1074) } else { (fraction | 1 << 52, biased - 1075) };
    let inclusive = significand % 2 == 0;
    // Doubled so that the halfway points are whole; at a power of two the
    // double below is half as far away, so the scale doubles again. (Core
    // also treats 2^-1022 so, and every subnormal as even; neither changes a
    // digit, since no decimal of 17 digits or fewer lies on those ends.)
    let (significand, plus, exponent) =
        if biased > 1 && fraction == 0 { (significand << 2, 2, exponent - 2) } else { (significand << 1, 1, exponent - 1) };
    let mut r = Big::from(significand);
    let mut s = Big::from(1);
    let mut m_minus = Big::from(1);
    let mut m_plus = Big::from(plus);
    if exponent >= 0 {
        r.mul_pow2(exponent as u32);
        m_minus.mul_pow2(exponent as u32);
        m_plus.mul_pow2(exponent as u32);
    } else {
        s.mul_pow2(exponent.unsigned_abs());
    }
    // 10^k bounds the interval's top; start from an estimate no larger than
    // it (log10 2 is a little over 1292913986 / 2^32) and step up.
    let top_bits = 64 - (significand + plus).leading_zeros() as i64 + i64::from(exponent);
    let mut k = (-((-(top_bits - 1) * 1_292_913_986) >> 32)) as i32;
    if k >= 0 {
        s.mul_pow10(k as u32);
    } else {
        r.mul_pow10(k.unsigned_abs());
        m_minus.mul_pow10(k.unsigned_abs());
        m_plus.mul_pow10(k.unsigned_abs());
    }
    while beyond_top(&r, &m_plus, &s, inclusive) {
        s.mul_pow10(1);
        k += 1;
    }
    // Every number the digits need is below 10s, so a value within 120 bits
    // of precision, as most are, runs in plain u128 arithmetic.
    let (digits, count) = if s.length < 4 || (s.length == 4 && s.limbs[3] < 1 << 24) {
        generate(r.low(), s.low(), m_minus.low(), m_plus.low(), inclusive)
    } else {
        generate(r, s, m_minus, m_plus, inclusive)
    };
    let mut text = TextBuffer::<24>::new();
    for &digit in &digits[..count] {
        text.push(digit);
    }
    text.trim_end(b'0', 1);
    (text, k - 1)
}

/// Whether `r + m_plus` reaches past `s`, the top of the interval.
fn beyond_top<N: Natural>(r: &N, m_plus: &N, s: &N, inclusive: bool) -> bool {
    let top = r.sum(m_plus).order(s);
    if inclusive { top.is_ge() } else { top.is_gt() }
}

/// The digits of `r / s`, which is below 1, up to the first that leaves the
/// value within its interval, rounded as core rounds them, and how many there
/// are. At most 17 come out: 17 always single out a double.
fn generate<N: Natural>(mut r: N, s: N, mut m_minus: N, mut m_plus: N, inclusive: bool) -> ([u8; 17], usize) {
    let mut digits = [0u8; 17];
    let mut count = 0;
    loop {
        r.times_ten();
        m_minus.times_ten();
        m_plus.times_ten();
        let mut digit = b'0';
        while r.order(&s).is_ge() {
            r.reduce(&s);
            digit += 1;
        }
        digits[count] = digit;
        count += 1;
        let low = if inclusive { r.order(&m_minus).is_le() } else { r.order(&m_minus).is_lt() };
        let high = beyond_top(&r, &m_plus, &s, inclusive);
        if !(low || high) {
            continue;
        }
        if high && (!low || r.sum(&r).order(&s).is_ge()) {
            // Round the last digit up; nines carry left. The carry stops at
            // the first digit: past it the value would be 10^k, which the
            // step-up of k left outside the interval.
            let mut last = count;
            while last > 1 && digits[last - 1] == b'9' {
                last -= 1;
            }
            digits[last - 1] += 1;
            count = last;
        }
        return (digits, count);
    }
}

/// The arithmetic the digit generation needs on non-negative integers.
trait Natural: Copy {
    fn times_ten(&mut self);
    fn order(&self, other: &Self) -> core::cmp::Ordering;
    fn sum(&self, other: &Self) -> Self;
    /// `self - other`. Requires: `self >= other`.
    fn reduce(&mut self, other: &Self);
}

impl Natural for u128 {
    fn times_ten(&mut self) {
        *self *= 10;
    }
    fn order(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
    fn sum(&self, other: &Self) -> Self {
        self + other
    }
    fn reduce(&mut self, other: &Self) {
        *self -= other;
    }
}

impl Natural for Big {
    fn times_ten(&mut self) {
        self.multiply(10);
    }
    fn order(&self, other: &Self) -> core::cmp::Ordering {
        self.compare(other)
    }
    fn sum(&self, other: &Self) -> Self {
        self.plus(other)
    }
    fn reduce(&mut self, other: &Self) {
        self.subtract(other);
    }
}

/// The limbs a value of the digit generation can need: the smallest
/// subnormal scaled by 10^324, times 10, is under 1,140 bits.
const LIMBS: usize = 36;

/// A non-negative integer of up to `LIMBS` 32-bit limbs, least significant
/// first; limbs at and past `length` are zero.
#[derive(Clone, Copy)]
struct Big {
    limbs: [u32; LIMBS],
    length: usize,
}

impl Big {
    fn from(value: u64) -> Big {
        let mut big = Big { limbs: [0; LIMBS], length: 2 };
        big.limbs[0] = value as u32;
        big.limbs[1] = (value >> 32) as u32;
        big.trim();
        big
    }

    /// The low 128 bits.
    fn low(&self) -> u128 {
        self.limbs[..4].iter().rev().fold(0, |value, &limb| value << 32 | u128::from(limb))
    }

    fn trim(&mut self) {
        while self.length > 0 && self.limbs[self.length - 1] == 0 {
            self.length -= 1;
        }
    }

    fn multiply(&mut self, factor: u32) {
        let mut carry = 0u64;
        for limb in &mut self.limbs[..self.length] {
            let product = u64::from(*limb) * u64::from(factor) + carry;
            *limb = product as u32;
            carry = product >> 32;
        }
        if carry != 0 {
            self.limbs[self.length] = carry as u32;
            self.length += 1;
        }
    }

    fn mul_pow10(&mut self, mut power: u32) {
        while power >= 9 {
            self.multiply(1_000_000_000);
            power -= 9;
        }
        self.multiply(10u32.pow(power));
    }

    fn mul_pow2(&mut self, power: u32) {
        let (words, bits) = ((power / 32) as usize, power % 32);
        if self.length == 0 {
            return;
        }
        self.limbs.copy_within(..self.length, words);
        self.limbs[..words].fill(0);
        self.length += words;
        if bits != 0 {
            let mut carry = 0;
            for limb in &mut self.limbs[words..self.length] {
                let shifted = (*limb << bits) | carry;
                carry = *limb >> (32 - bits);
                *limb = shifted;
            }
            if carry != 0 {
                self.limbs[self.length] = carry;
                self.length += 1;
            }
        }
    }

    fn plus(&self, other: &Big) -> Big {
        let mut sum = *self;
        let length = self.length.max(other.length);
        let mut carry = 0u64;
        for (limb, &addend) in sum.limbs[..length].iter_mut().zip(&other.limbs[..length]) {
            let total = u64::from(*limb) + u64::from(addend) + carry;
            *limb = total as u32;
            carry = total >> 32;
        }
        sum.length = length;
        if carry != 0 {
            sum.limbs[length] = 1;
            sum.length += 1;
        }
        sum
    }

    /// `self - other`. Requires: `self >= other`.
    fn subtract(&mut self, other: &Big) {
        let mut borrow = 0i64;
        for (limb, &subtrahend) in self.limbs[..self.length].iter_mut().zip(&other.limbs[..self.length]) {
            let difference = i64::from(*limb) - i64::from(subtrahend) - borrow;
            *limb = difference as u32;
            borrow = i64::from(difference < 0);
        }
        self.trim();
    }

    fn compare(&self, other: &Big) -> core::cmp::Ordering {
        self.length.cmp(&other.length).then_with(|| self.limbs[..self.length].iter().rev().cmp(other.limbs[..self.length].iter().rev()))
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
