use crate::decimal::{self, Decimal};
use std::collections::HashSet;
use std::fmt::Write;

const MAX_JSON_DEPTH: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum JsonData {
    Null,
    Boolean(bool),
    Number(String),
    Text(String),
    Array(Vec<JsonData>),
    Object(Vec<(String, JsonData)>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JsonDecodeError {
    pub(crate) message: String,
    pub(crate) offset: usize,
}

pub(crate) fn decode_json(source: &str) -> Result<JsonData, JsonDecodeError> {
    JsonParser {
        source,
        bytes: source.as_bytes(),
        offset: 0,
    }
    .parse()
}

pub(crate) fn write_json_data(output: &mut String, value: &JsonData) {
    match value {
        JsonData::Null => output.push_str("null"),
        JsonData::Boolean(value) => output.push_str(if *value { "true" } else { "false" }),
        JsonData::Number(value) => output.push_str(value),
        JsonData::Text(value) => write_json_string(output, value),
        JsonData::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json_data(output, value);
            }
            output.push(']');
        }
        JsonData::Object(fields) => {
            let mut fields = fields.iter().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            output.push('{');
            for (index, (name, value)) in fields.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json_string(output, name);
                output.push(':');
                write_json_data(output, value);
            }
            output.push('}');
        }
    }
}

/// Writes `value` as a JSON string as RFC 8259 asks, for `Json.encode` and runtime
/// records: only U+0000 to U+001F are escaped, as the native runtime does.
pub(crate) fn write_json_string(output: &mut String, value: &str) {
    write_escaped(output, value, |character| character < '\u{20}');
}

/// Writes `value` as a JSON string with every Unicode control character escaped,
/// DEL and C1 included, for tool output such as inspection.
pub(crate) fn write_json_string_escaping_controls(output: &mut String, value: &str) {
    write_escaped(output, value, char::is_control);
}

fn write_escaped(output: &mut String, value: &str, escaped: impl Fn(char) -> bool) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if escaped(character) => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing JSON to a string cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

struct JsonParser<'source> {
    source: &'source str,
    bytes: &'source [u8],
    offset: usize,
}

impl JsonParser<'_> {
    fn parse(mut self) -> Result<JsonData, JsonDecodeError> {
        self.skip_whitespace();
        let value = self.value(0)?;
        self.skip_whitespace();
        if self.offset != self.bytes.len() {
            return Err(self.error("unexpected characters after JSON value"));
        }
        Ok(value)
    }

    fn value(&mut self, depth: usize) -> Result<JsonData, JsonDecodeError> {
        if depth > MAX_JSON_DEPTH {
            return Err(self.error("JSON nesting exceeds the configured limit"));
        }
        match self.peek() {
            Some(b'n') => {
                self.keyword(b"null")?;
                Ok(JsonData::Null)
            }
            Some(b't') => {
                self.keyword(b"true")?;
                Ok(JsonData::Boolean(true))
            }
            Some(b'f') => {
                self.keyword(b"false")?;
                Ok(JsonData::Boolean(false))
            }
            Some(b'"') => self.string().map(JsonData::Text),
            Some(b'[') => self.array(depth + 1),
            Some(b'{') => self.object(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.number().map(JsonData::Number),
            Some(_) => Err(self.error("expected a JSON value")),
            None => Err(self.error("expected a JSON value")),
        }
    }

    fn keyword(&mut self, keyword: &[u8]) -> Result<(), JsonDecodeError> {
        if self.bytes.get(self.offset..self.offset + keyword.len()) == Some(keyword) {
            self.offset += keyword.len();
            Ok(())
        } else {
            Err(self.error("invalid JSON literal"))
        }
    }

    fn array(&mut self, depth: usize) -> Result<JsonData, JsonDecodeError> {
        self.offset += 1;
        self.skip_whitespace();
        let mut values = Vec::new();
        if self.take(b']') {
            return Ok(JsonData::Array(values));
        }
        loop {
            values.push(self.value(depth)?);
            self.skip_whitespace();
            if self.take(b']') {
                return Ok(JsonData::Array(values));
            }
            if !self.take(b',') {
                return Err(self.error("expected `,` or `]` in JSON array"));
            }
            self.skip_whitespace();
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonData, JsonDecodeError> {
        self.offset += 1;
        self.skip_whitespace();
        let mut fields = Vec::new();
        let mut names = HashSet::new();
        if self.take(b'}') {
            return Ok(JsonData::Object(fields));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a string key in JSON object"));
            }
            let name_offset = self.offset;
            let name = self.string()?;
            if !names.insert(name.clone()) {
                return Err(JsonDecodeError {
                    message: format!("duplicate JSON object key `{name}`"),
                    offset: name_offset,
                });
            }
            self.skip_whitespace();
            if !self.take(b':') {
                return Err(self.error("expected `:` after JSON object key"));
            }
            self.skip_whitespace();
            let value = self.value(depth)?;
            fields.push((name, value));
            self.skip_whitespace();
            if self.take(b'}') {
                return Ok(JsonData::Object(fields));
            }
            if !self.take(b',') {
                return Err(self.error("expected `,` or `}` in JSON object"));
            }
            self.skip_whitespace();
        }
    }

    fn number(&mut self) -> Result<String, JsonDecodeError> {
        let start = self.offset;
        self.take(b'-');
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.error("JSON numbers cannot contain leading zeroes"));
                }
            }
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.error("expected digits in JSON number")),
        }
        if self.take(b'.') {
            let fraction = self.offset;
            self.digits();
            if self.offset == fraction {
                return Err(self.error("expected digits after JSON decimal point"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            let exponent = self.offset;
            self.digits();
            if self.offset == exponent {
                return Err(self.error("expected digits in JSON exponent"));
            }
        }
        Ok(self.source[start..self.offset].to_owned())
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
    }

    fn string(&mut self) -> Result<String, JsonDecodeError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.offset += 1;
        let mut value = String::new();
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.offset += 1;
                    return Ok(value);
                }
                Some(b'\\') => {
                    self.offset += 1;
                    self.escape(&mut value)?;
                }
                Some(0x00..=0x1f) => {
                    return Err(self.error("unescaped control character in JSON string"));
                }
                Some(_) => {
                    let character = self.source[self.offset..]
                        .chars()
                        .next()
                        .expect("offset remains on a UTF-8 boundary");
                    self.offset += character.len_utf8();
                    value.push(character);
                }
                None => return Err(self.error("unterminated JSON string")),
            }
        }
    }

    fn escape(&mut self, output: &mut String) -> Result<(), JsonDecodeError> {
        let Some(escape) = self.peek() else {
            return Err(self.error("unterminated JSON escape"));
        };
        self.offset += 1;
        match escape {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{8}'),
            b'f' => output.push('\u{c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => {
                let first = self.hex_quad()?;
                let scalar = if (0xd800..=0xdbff).contains(&first) {
                    if !self.take(b'\\') || !self.take(b'u') {
                        return Err(
                            self.error("high surrogate must be followed by a low surrogate")
                        );
                    }
                    let second = self.hex_quad()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(
                            self.error("high surrogate must be followed by a low surrogate")
                        );
                    }
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return Err(self.error("isolated low surrogate in JSON string"));
                } else {
                    u32::from(first)
                };
                output.push(char::from_u32(scalar).expect("validated JSON scalar"));
            }
            _ => return Err(self.error("invalid JSON escape")),
        }
        Ok(())
    }

    fn hex_quad(&mut self) -> Result<u16, JsonDecodeError> {
        let start = self.offset;
        let end = start.saturating_add(4);
        let Some(bytes) = self.bytes.get(start..end) else {
            return Err(self.error("incomplete JSON Unicode escape"));
        };
        let mut value = 0_u16;
        for byte in bytes {
            value = value
                .checked_mul(16)
                .expect("four hexadecimal digits fit in u16");
            value += match byte {
                b'0'..=b'9' => u16::from(byte - b'0'),
                b'a'..=b'f' => u16::from(byte - b'a' + 10),
                b'A'..=b'F' => u16::from(byte - b'A' + 10),
                _ => return Err(self.error("invalid JSON Unicode escape")),
            };
        }
        self.offset = end;
        Ok(value)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.offset += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn take(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn error(&self, message: &str) -> JsonDecodeError {
        JsonDecodeError {
            message: message.to_owned(),
            offset: self.offset,
        }
    }
}

/// A JSON number in canonical decimal form: `sign × 0.digits × 10^exponent`, with
/// no leading or trailing zeros in `digits`. Zero has sign 0 and empty digits.
struct CanonicalNumber {
    sign: i8,
    digits: Vec<u8>,
    exponent: Exponent,
}

/// A signed decimal integer of any size. A written exponent may have more digits
/// than any machine integer holds, and numbers that differ only there still
/// differ, so the exponent is never clamped or wrapped.
struct Exponent {
    negative: bool,
    /// Decimal digit values, most significant first, with no leading zeros;
    /// empty for zero, which is never negative.
    magnitude: Vec<u8>,
}

impl Exponent {
    fn new(negative: bool, digits: &[u8]) -> Self {
        let leading = digits.iter().take_while(|digit| **digit == 0).count();
        let magnitude = digits[leading..].to_vec();
        Exponent {
            negative: negative && !magnitude.is_empty(),
            magnitude,
        }
    }

    fn from_offset(offset: i64) -> Self {
        let digits = offset.unsigned_abs().to_string().into_bytes();
        let digits = digits.iter().map(|digit| digit - b'0').collect::<Vec<_>>();
        Exponent::new(offset < 0, &digits)
    }

    fn plus(&self, other: &Exponent) -> Exponent {
        use std::cmp::Ordering::{Equal, Greater, Less};
        if self.negative == other.negative {
            let sum = add_magnitudes(&self.magnitude, &other.magnitude);
            return Exponent::new(self.negative, &sum);
        }
        match compare_magnitudes(&self.magnitude, &other.magnitude) {
            Equal => Exponent::new(false, &[]),
            Greater => Exponent::new(
                self.negative,
                &subtract_magnitudes(&self.magnitude, &other.magnitude),
            ),
            Less => Exponent::new(
                other.negative,
                &subtract_magnitudes(&other.magnitude, &self.magnitude),
            ),
        }
    }

    fn compare(&self, other: &Exponent) -> std::cmp::Ordering {
        let rank = |exponent: &Exponent| {
            if exponent.negative {
                -1
            } else {
                i8::from(!exponent.magnitude.is_empty())
            }
        };
        rank(self).cmp(&rank(other)).then_with(|| {
            let magnitude = compare_magnitudes(&self.magnitude, &other.magnitude);
            if self.negative {
                magnitude.reverse()
            } else {
                magnitude
            }
        })
    }
}

fn compare_magnitudes(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn add_magnitudes(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut sum = Vec::with_capacity(left.len().max(right.len()) + 1);
    let (mut left, mut right) = (left.iter().rev(), right.iter().rev());
    let mut carry = 0;
    loop {
        let (a, b) = (left.next(), right.next());
        if a.is_none() && b.is_none() {
            break;
        }
        let total = a.copied().unwrap_or(0) + b.copied().unwrap_or(0) + carry;
        sum.push(total % 10);
        carry = total / 10;
    }
    if carry > 0 {
        sum.push(carry);
    }
    sum.reverse();
    sum
}

/// Requires: `larger` is at least `smaller`.
fn subtract_magnitudes(larger: &[u8], smaller: &[u8]) -> Vec<u8> {
    let mut difference = Vec::with_capacity(larger.len());
    let mut smaller = smaller.iter().rev();
    let mut borrow = 0;
    for digit in larger.iter().rev() {
        let subtrahend = smaller.next().copied().unwrap_or(0) + borrow;
        borrow = u8::from(*digit < subtrahend);
        difference.push(digit + 10 * borrow - subtrahend);
    }
    difference.reverse();
    difference
}

fn canonical_number(text: &str) -> CanonicalNumber {
    let bytes = text.as_bytes();
    let (negative, rest) = match bytes.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, bytes),
    };
    let mantissa_end = rest
        .iter()
        .position(|byte| matches!(byte, b'e' | b'E'))
        .unwrap_or(rest.len());
    let (mantissa, exponent_part) = rest.split_at(mantissa_end);
    let written = match exponent_part.split_first() {
        None => Exponent::new(false, &[]),
        Some((_, written)) => {
            let (negative_exponent, digits) = match written.split_first() {
                Some((b'-', digits)) => (true, digits),
                Some((b'+', digits)) => (false, digits),
                _ => (false, written),
            };
            let digits = digits.iter().map(|digit| digit - b'0').collect::<Vec<_>>();
            Exponent::new(negative_exponent, &digits)
        }
    };
    let point = mantissa
        .iter()
        .position(|byte| *byte == b'.')
        .unwrap_or(mantissa.len());
    let mut digits = mantissa
        .iter()
        .filter(|byte| byte.is_ascii_digit())
        .copied()
        .collect::<Vec<_>>();
    let leading = digits.iter().take_while(|digit| **digit == b'0').count();
    digits.drain(..leading);
    while digits.last() == Some(&b'0') {
        digits.pop();
    }
    if digits.is_empty() {
        return CanonicalNumber {
            sign: 0,
            digits,
            exponent: Exponent::new(false, &[]),
        };
    }
    CanonicalNumber {
        sign: if negative { -1 } else { 1 },
        digits,
        exponent: written.plus(&Exponent::from_offset(point as i64 - leading as i64)),
    }
}

/// Orders two JSON number texts by the exact numbers they denote, never through
/// `f64`: `1`, `1.0`, and `10e-1` are equal, and `-0` equals `0`.
///
/// Requires: both texts match the JSON number grammar.
pub(crate) fn compare_numbers(left: &str, right: &str) -> std::cmp::Ordering {
    let (left, right) = (canonical_number(left), canonical_number(right));
    left.sign.cmp(&right.sign).then_with(|| {
        let magnitude = left
            .exponent
            .compare(&right.exponent)
            .then_with(|| left.digits.cmp(&right.digits));
        if left.sign < 0 {
            magnitude.reverse()
        } else {
            magnitude
        }
    })
}

/// Why a JSON number has no value of the target type (`std/json.ConversionError`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NumberConversionError {
    NotInteger,
    OutOfRange,
}

/// Written exponents are clamped to this magnitude. Every number whose exponent
/// reaches it is far outside `Int` and `Decimal`, and any spelling the decoder
/// can hold has fewer digits than this, so the clamp never changes a result
/// while keeping the arithmetic below in `i64`.
const EXPONENT_CLAMP: i64 = 1_000_000_000_000_000;

/// The exact value of a JSON number as `± significant × 10^exponent`, where
/// `significant` has no leading or trailing zeros (empty for zero), and the
/// scale its spelling writes: fraction digits minus the written exponent.
struct ExactNumber {
    negative: bool,
    significant: Vec<u8>,
    exponent: i64,
    written_scale: i64,
}

/// Requires: `text` matches the JSON number grammar.
fn exact_number(text: &str) -> ExactNumber {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (mantissa, written) = rest.split_once(['e', 'E']).unwrap_or((rest, ""));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let written = written.strip_prefix('+').unwrap_or(written);
    let (exponent_negative, exponent_digits) = match written.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, written),
    };
    let magnitude = exponent_digits.bytes().fold(0_i64, |magnitude, digit| {
        (magnitude * 10 + i64::from(digit - b'0')).min(EXPONENT_CLAMP)
    });
    let written_exponent = if exponent_negative { -magnitude } else { magnitude };
    let fraction_digits = i64::try_from(fraction.len()).expect("a spelling fits in memory");
    let digits = whole.bytes().chain(fraction.bytes()).map(|digit| digit - b'0');
    let mut significant = digits.skip_while(|digit| *digit == 0).collect::<Vec<_>>();
    let mut trailing = 0;
    while significant.last() == Some(&0) {
        significant.pop();
        trailing += 1;
    }
    ExactNumber {
        negative,
        significant,
        exponent: written_exponent - fraction_digits + trailing,
        written_scale: fraction_digits - written_exponent,
    }
}

/// `Json.number_to_int`: the `Int` a JSON number denotes, whatever its spelling.
///
/// Requires: `text` matches the JSON number grammar.
pub(crate) fn number_to_int(text: &str) -> Result<i64, NumberConversionError> {
    let number = exact_number(text);
    if number.significant.is_empty() {
        return Ok(0);
    }
    if number.exponent < 0 {
        return Err(NumberConversionError::NotInteger);
    }
    let length = i64::try_from(number.significant.len()).expect("a spelling fits in memory");
    if length + number.exponent > 19 {
        return Err(NumberConversionError::OutOfRange);
    }
    let zeros = std::iter::repeat_n(0, number.exponent as usize);
    let magnitude = number
        .significant
        .iter()
        .copied()
        .chain(zeros)
        .fold(0_i128, |value, digit| value * 10 + i128::from(digit));
    let value = if number.negative { -magnitude } else { magnitude };
    i64::try_from(value).map_err(|_| NumberConversionError::OutOfRange)
}

/// `Json.number_to_decimal`: the exact `Decimal` at the spelling's own scale,
/// or the largest smaller scale that fits when it does not; `None` when the
/// value needs more than 38 significant or fraction digits.
///
/// Requires: `text` matches the JSON number grammar.
pub(crate) fn number_to_decimal(text: &str) -> Option<Decimal> {
    let number = exact_number(text);
    let max_scale = i64::from(decimal::MAX_SCALE);
    if number.significant.is_empty() {
        let scale = number.written_scale.clamp(0, max_scale);
        return Decimal::new(false, 0, scale as u32);
    }
    let length = i64::try_from(number.significant.len()).expect("a spelling fits in memory");
    if length + number.exponent.max(0) > max_scale || -number.exponent > max_scale {
        return None;
    }
    // At least the smallest scale, `max(0, -exponent)`: the written scale is
    // `-exponent` plus the trailing zeros, and the checks above keep both upper
    // bounds above it. A negative written scale, as in `1.5e3`, is scale 0.
    let scale = number
        .written_scale
        .max(0)
        .min(max_scale)
        .min(max_scale - length - number.exponent);
    let zeros = std::iter::repeat_n(0, (number.exponent + scale) as usize);
    let magnitude = number
        .significant
        .iter()
        .copied()
        .chain(zeros)
        .fold(0_u128, |value, digit| value * 10 + u128::from(digit));
    Decimal::new(number.negative, magnitude, scale as u32)
}

#[cfg(test)]
mod tests {
    use super::{
        JsonData, NumberConversionError, compare_numbers, decode_json, number_to_decimal,
        number_to_int, write_json_data, write_json_string, write_json_string_escaping_controls,
    };
    use crate::decimal::Decimal;

    #[test]
    fn numbers_convert_to_int_by_the_value_they_denote() {
        use NumberConversionError::{NotInteger, OutOfRange};
        let cases = [
            ("0", Ok(0)),
            ("-0", Ok(0)),
            ("0.000e-99999999999999999999", Ok(0)),
            ("1.0", Ok(1)),
            ("1e2", Ok(100)),
            ("1500e-2", Ok(15)),
            ("-12.5e1", Ok(-125)),
            ("9223372036854775807", Ok(i64::MAX)),
            ("-9223372036854775808", Ok(i64::MIN)),
            ("922337203685477580.7e1", Ok(i64::MAX)),
            ("9223372036854775808", Err(OutOfRange)),
            ("-9223372036854775809", Err(OutOfRange)),
            ("1e19", Err(OutOfRange)),
            ("1.5e400", Err(OutOfRange)),
            ("1e99999999999999999999", Err(OutOfRange)),
            ("1.5", Err(NotInteger)),
            ("1e-1", Err(NotInteger)),
            ("12345678901234567890.5", Err(NotInteger)),
            ("1e-99999999999999999999", Err(NotInteger)),
        ];
        for (text, expected) in cases {
            assert_eq!(number_to_int(text), expected, "{text}");
        }
    }

    #[test]
    fn numbers_convert_to_decimal_exactly_at_their_written_scale() {
        let cases = [
            ("1.50", Some("1.50")),
            ("-0.0", Some("0.0")),
            ("0e-50", Some("0.00000000000000000000000000000000000000")),
            ("0e50", Some("0")),
            ("1.5e3", Some("1500")),
            ("1500e-2", Some("15.00")),
            ("123456789012345678901234567890", Some("123456789012345678901234567890")),
            (
                "0.12345678901234567890123456789012345678",
                Some("0.12345678901234567890123456789012345678"),
            ),
            (
                "-9.9999999999999999999999999999999999999e37",
                Some("-99999999999999999999999999999999999999"),
            ),
            // The written scale is 40; dropping trailing zeros keeps it exact.
            (
                "1.0000000000000000000000000000000000000000",
                Some("1.0000000000000000000000000000000000000"),
            ),
            ("1e38", None),
            ("0.123456789012345678901234567890123456789", None),
            ("1e-39", None),
            ("1e99999999999999999999", None),
            ("1e-99999999999999999999", None),
        ];
        for (text, expected) in cases {
            assert_eq!(
                number_to_decimal(text).map(Decimal::to_text).as_deref(),
                expected,
                "{text}"
            );
        }
    }

    #[test]
    fn numbers_compare_by_the_exact_value_they_denote() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        let cases = [
            ("1", "1.0", Equal),
            ("1", "10e-1", Equal),
            ("100", "1E2", Equal),
            ("0.5", "5e-1", Equal),
            ("1.5", "1.50", Equal),
            ("-0", "0", Equal),
            ("0.000", "-0e5", Equal),
            ("2", "10", Less),
            ("0.25", "0.3", Less),
            ("-2", "-10", Greater),
            ("-1", "0", Less),
            ("1e400", "1e399", Greater),
            ("12345678901234567890123", "12345678901234567890124", Less),
            ("0.1", "0.09999999999999999999", Greater),
            // Exponents beyond any machine integer stay exact.
            ("1e9999999999999999999", "1", Greater),
            ("1e9999999999999999999", "1e9999999999999999998", Greater),
            ("1e-9999999999999999999", "1e-9999999999999999998", Less),
            ("1e1152921504606846977", "1e1152921504606846976", Greater),
            ("10e1152921504606846976", "1e1152921504606846977", Equal),
            (
                "0.01e100000000000000000002",
                "1e100000000000000000000",
                Equal,
            ),
            (
                "-1e99999999999999999999999",
                "-1e99999999999999999999998",
                Less,
            ),
            ("1e-00000000000000000000001", "0.1", Equal),
            ("0e99999999999999999999999", "-0", Equal),
            ("1e9223372036854775807", "10e9223372036854775806", Equal),
            ("1e9223372036854775808", "1e9223372036854775807", Greater),
            (
                "0.001e-9223372036854775807",
                "1e-9223372036854775810",
                Equal,
            ),
            ("100e9999999999999999999", "1e10000000000000000001", Equal),
            ("0.001e10000000000000000000", "1e9999999999999999997", Equal),
        ];
        for (left, right, expected) in cases {
            assert_eq!(compare_numbers(left, right), expected, "{left} vs {right}");
            assert_eq!(
                compare_numbers(right, left),
                expected.reverse(),
                "{right} vs {left}"
            );
        }
    }

    #[test]
    fn strings_escape_json_control_characters() {
        let mut output = String::new();
        write_json_string(&mut output, "quote\" slash\\ line\n\t\u{1}");

        assert_eq!(output, "\"quote\\\" slash\\\\ line\\n\\t\\u0001\"");
    }

    #[test]
    fn encode_escapes_only_c0_controls_and_inspection_escapes_every_control() {
        let mut encoded = String::new();
        write_json_data(&mut encoded, &JsonData::Text("del\u{7f} nel\u{85} \u{1f}".to_owned()));
        assert_eq!(encoded, "\"del\u{7f} nel\u{85} \\u001f\"");

        let mut inspected = String::new();
        write_json_string_escaping_controls(&mut inspected, "del\u{7f} nel\u{85} \u{1f}");
        assert_eq!(inspected, "\"del\\u007f nel\\u0085 \\u001f\"");
    }

    #[test]
    fn decode_rejects_duplicate_keys_and_invalid_numbers() {
        assert!(decode_json("{\"a\":1,\"a\":2}").is_err());
        for source in ["01", "1.", "1e", "-"] {
            assert!(decode_json(source).is_err(), "{source}");
        }
    }

    #[test]
    fn object_encoding_sorts_utf8_keys_recursively() {
        let value = JsonData::Object(vec![
            ("z".to_owned(), JsonData::Null),
            (
                "a".to_owned(),
                JsonData::Object(vec![
                    ("β".to_owned(), JsonData::Boolean(true)),
                    ("b".to_owned(), JsonData::Number("2".to_owned())),
                ]),
            ),
        ]);
        let mut output = String::new();
        write_json_data(&mut output, &value);
        assert_eq!(output, "{\"a\":{\"b\":2,\"β\":true},\"z\":null}");
    }

    #[test]
    fn unicode_surrogate_pairs_decode_to_one_scalar() {
        assert_eq!(
            decode_json("\"\\ud83d\\udca5\"").unwrap(),
            JsonData::Text("💥".to_owned())
        );
    }

    #[test]
    fn current_runtime_event_and_outcome_envelopes_decode_as_objects() {
        let records = [
            r#"{"schema":"chimi.runtime.v1","event":"computation_started","sequence":0,"computation":"main","execution_id":"run-1","span_id":1,"parent_span_id":null,"core_node_id":0,"origin_id":0,"semantic_id":0,"file_id":0,"source_start":0,"source_end":1,"operation":"computation","attributes":{}}"#,
            r#"{"schema":"chimi.runtime.v1","event":"operation_started","sequence":1,"computation":"main","execution_id":"run-1","span_id":2,"parent_span_id":1,"core_node_id":0,"origin_id":0,"semantic_id":0,"file_id":0,"source_start":0,"source_end":1,"operation":"call","attributes":{}}"#,
            r#"{"schema":"chimi.runtime.v1","event":"dependency_called","sequence":2,"computation":"main","execution_id":"run-1","span_id":3,"parent_span_id":2,"core_node_id":0,"origin_id":0,"semantic_id":0,"file_id":0,"source_start":0,"source_end":1,"operation":"dependency_call","attributes":{"requirement":"payment","requirement_symbol_id":1,"interface_symbol_id":2,"method":"charge","method_semantic_id":3}}"#,
            r#"{"schema":"chimi.runtime.v1","event":"operation_completed","sequence":3,"computation":"main","execution_id":"run-1","span_id":2,"parent_span_id":1,"core_node_id":0,"origin_id":0,"semantic_id":0,"file_id":0,"source_start":0,"source_end":1,"operation":"call","attributes":{"outcome":"success"}}"#,
            r#"{"schema":"chimi.runtime.v1","outcome":"success","value":42}"#,
            r#"{"schema":"chimi.runtime.v1","outcome":"expected_failure","error":{"type":"PaymentError","variant":"Declined","fields":{}}}"#,
            r#"{"schema":"chimi.runtime.v1","outcome":"defect","message":"overflow","core_node_id":4,"origin_id":2}"#,
            r#"{"schema":"chimi.runtime.v1","outcome":"cancellation","reason":"requested before the next semantic operation"}"#,
        ];

        for record in records {
            assert!(matches!(decode_json(record), Ok(JsonData::Object(_))));
        }
    }

    #[test]
    fn decode_enforces_the_nesting_limit() {
        let source = format!("{}null{}", "[".repeat(129), "]".repeat(129));
        let error = decode_json(&source).unwrap_err();

        assert!(error.message.contains("nesting"));
    }
}
