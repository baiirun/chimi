//! The intrinsics over single values that `runtime.c` kept in its own
//! switch: std/path's identities, `List.first` and `List.length`, std/order,
//! `Text.append` and `Text.equal`, std/int's parsing, text, checked
//! arithmetic and modulo, std/range, std/float's conversions, text and
//! parsing, and `Debug.render`. `execution/core_operations.rs` and
//! `numeric.rs` are the interpreter's, and the results and defect messages
//! match them. Values are allocated in the order `runtime.c` allocated them,
//! so memory reports do not change.

use super::debug::render;
use super::message::{Message, integer_spelling};
use super::standard::{boolean_value, defect, defect_with, failure, option_value, shape_defect, standard_value, success};
use super::text::HeapText;
use crate::float;
use crate::heap::compare::compare;
use crate::heap::layout::{Control, Intrinsic, StandardConstructor, Value, ValueKind};
use crate::heap::list;
use crate::heap::values::{float_value, integer_value, text_value};

unsafe extern "C" {
    // The C library's math (libm, which every program links).
    fn trunc(value: f64) -> f64;
    fn floor(value: f64) -> f64;
    fn ceil(value: f64) -> f64;
    fn round(value: f64) -> f64;
}

/// An Int.
///
/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn int(raw: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::INTEGER, raw) }
}

/// A Float.
///
/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn float(raw: f64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { float_value(raw) }
}

/// A Text holding a copy of `bytes`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn new_text(bytes: &[u8]) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { text_value(bytes.as_ptr().cast(), bytes.len()) }
}

/// The bytes of a Text value.
///
/// # Safety
/// `value` is a live Text.
unsafe fn bytes_of<'a>(value: *const Value) -> &'a [u8] {
    // SAFETY: the caller's contract; the buffer is never null.
    unsafe {
        let payload = (*value).payload.text;
        core::slice::from_raw_parts(payload.bytes.cast::<u8>(), payload.length)
    }
}

/// A standard constructor without fields.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn constant(constructor: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { standard_value(constructor, &[]) }
}

/// `Int.parse`: an optional `-`, then ASCII digits that fit in an Int.
fn parse_int(text: &[u8]) -> Result<i64, u32> {
    if text.is_empty() {
        return Err(StandardConstructor::INT_PARSE_EMPTY);
    }
    let (negative, digits) = match text {
        [b'-', rest @ ..] => (true, rest),
        rest => (false, rest),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) || text.contains(&0) {
        return Err(StandardConstructor::INT_PARSE_INVALID_DIGIT);
    }
    // Accumulated negative, so i64::MIN parses.
    let mut value: i64 = 0;
    for &digit in digits {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_sub(i64::from(digit - b'0')))
            .ok_or(StandardConstructor::INT_PARSE_OUT_OF_RANGE)?;
    }
    if negative { Ok(value) } else { value.checked_neg().ok_or(StandardConstructor::INT_PARSE_OUT_OF_RANGE) }
}

/// `digits [. digits] [(e|E) [+|-] digits]`, as numeric.rs `is_decimal_number`.
fn is_decimal_number(text: &[u8]) -> bool {
    let mut cursor = 0;
    let digits = |cursor: &mut usize| {
        let start = *cursor;
        while text.get(*cursor).is_some_and(u8::is_ascii_digit) {
            *cursor += 1;
        }
        *cursor != start
    };
    if !digits(&mut cursor) {
        return false;
    }
    if text.get(cursor) == Some(&b'.') {
        cursor += 1;
        if !digits(&mut cursor) {
            return false;
        }
    }
    if matches!(text.get(cursor), Some(b'e' | b'E')) {
        cursor += 1;
        if matches!(text.get(cursor), Some(b'+' | b'-')) {
            cursor += 1;
        }
        if !digits(&mut cursor) {
            return false;
        }
    }
    cursor == text.len()
}

/// `Float.parse`, through the C library's correctly rounded `strtod`.
///
/// # Safety
/// `text` is a live Text, which keeps a NUL after its bytes.
unsafe fn parse_float(text: *const Value) -> Result<f64, u32> {
    // SAFETY: the caller's contract.
    unsafe {
        let bytes = bytes_of(text);
        if bytes.is_empty() {
            return Err(StandardConstructor::FLOAT_PARSE_EMPTY);
        }
        let unsigned = bytes.strip_prefix(b"-").unwrap_or(bytes);
        if !is_decimal_number(unsigned) {
            return Err(StandardConstructor::FLOAT_PARSE_INVALID);
        }
        let value = libc::strtod((*text).payload.text.bytes, core::ptr::null_mut());
        if value.is_finite() { Ok(value) } else { Err(StandardConstructor::FLOAT_PARSE_OUT_OF_RANGE) }
    }
}

/// `Int.add_checked` and its siblings: None when the result does not fit or
/// the divisor is zero.
fn checked(intrinsic: u32, left: i64, right: i64) -> Option<i64> {
    match intrinsic {
        Intrinsic::INT_ADD_CHECKED => left.checked_add(right),
        Intrinsic::INT_SUBTRACT_CHECKED => left.checked_sub(right),
        Intrinsic::INT_MULTIPLY_CHECKED => left.checked_mul(right),
        Intrinsic::INT_DIVIDE_CHECKED => left.checked_div(right),
        // i64::MIN / -1 overflows, but i64::MIN % -1 is exactly 0.
        _ if right == -1 => Some(0),
        _ => left.checked_rem(right),
    }
}

/// The intrinsic `intrinsic` of this family on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn scalar_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    use ValueKind as K;
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let kinds = |expected: &[ValueKind]| {
            arguments.len() == expected.len() && arguments.iter().zip(expected).all(|(&value, &kind)| (*value).kind == kind)
        };
        match intrinsic {
            Intrinsic::IO_PATH | Intrinsic::PATH_FROM | Intrinsic::PATH_TO_TEXT if kinds(&[K::TEXT]) => {
                success(arguments[0])
            }
            Intrinsic::LIST_FIRST if kinds(&[K::LIST]) => {
                let list = arguments[0];
                option_value(if (*list).payload.list.length == 0 { core::ptr::null_mut() } else { list::get(list, 0) })
            }
            Intrinsic::LIST_LENGTH if kinds(&[K::LIST]) => success(int(i64::from((*arguments[0]).payload.list.length))),
            Intrinsic::ORDER_COMPARE if arguments.len() == 2 => {
                let constructor = match compare(arguments[0], arguments[1]) {
                    ..0 => StandardConstructor::ORDERING_LESS,
                    0 => StandardConstructor::ORDERING_EQUAL,
                    _ => StandardConstructor::ORDERING_GREATER,
                };
                success(constant(constructor))
            }
            // Both return the left operand when the two are equal.
            Intrinsic::ORDER_MIN if arguments.len() == 2 => {
                success(if compare(arguments[0], arguments[1]) > 0 { arguments[1] } else { arguments[0] })
            }
            Intrinsic::ORDER_MAX if arguments.len() == 2 => {
                success(if compare(arguments[0], arguments[1]) < 0 { arguments[1] } else { arguments[0] })
            }
            Intrinsic::TEXT_APPEND if kinds(&[K::TEXT, K::TEXT]) => {
                let mut output = HeapText::new();
                output.push_bytes(bytes_of(arguments[0]));
                output.push_bytes(bytes_of(arguments[1]));
                success(new_text(output.bytes()))
            }
            Intrinsic::TEXT_EQUAL if kinds(&[K::TEXT, K::TEXT]) => {
                success(boolean_value(bytes_of(arguments[0]) == bytes_of(arguments[1])))
            }
            Intrinsic::INT_PARSE if kinds(&[K::TEXT]) => match parse_int(bytes_of(arguments[0])) {
                Ok(value) => success(int(value)),
                Err(constructor) => failure(constant(constructor)),
            },
            Intrinsic::INT_TO_TEXT if kinds(&[K::INTEGER]) => {
                let mut spelling = [0; 20];
                success(new_text(integer_spelling((*arguments[0]).payload.integer, &mut spelling)))
            }
            Intrinsic::INT_ADD_CHECKED..=Intrinsic::INT_REMAINDER_CHECKED if kinds(&[K::INTEGER, K::INTEGER]) => {
                let (left, right) = ((*arguments[0]).payload.integer, (*arguments[1]).payload.integer);
                match checked(intrinsic, left, right) {
                    Some(value) => option_value(int(value)),
                    None => option_value(core::ptr::null_mut()),
                }
            }
            Intrinsic::INT_MODULO if kinds(&[K::INTEGER, K::INTEGER]) => {
                let (value, modulus) = ((*arguments[0]).payload.integer, (*arguments[1]).payload.integer);
                if modulus == 0 {
                    let message = Message::new()
                        .text(b"integer division by zero in `Int.modulo`: Int.modulo(")
                        .integer(value)
                        .text(b", ")
                        .integer(modulus)
                        .text(b")");
                    return defect_with(message.finish(), node);
                }
                let mut remainder = if modulus == -1 { 0 } else { value % modulus };
                if remainder != 0 && (remainder < 0) != (modulus < 0) {
                    remainder += modulus;
                }
                success(int(remainder))
            }
            Intrinsic::RANGE_START..=Intrinsic::RANGE_LENGTH if kinds(&[K::RANGE]) => {
                let range = (*arguments[0]).payload.range;
                match intrinsic {
                    Intrinsic::RANGE_START => success(int(range.start)),
                    Intrinsic::RANGE_END => success(int(range.end)),
                    _ if range.end <= range.start => success(int(0)),
                    _ => match range.end.checked_sub(range.start) {
                        Some(length) => success(int(length)),
                        None => defect(
                            c"Range.length overflows Int: the range holds more than the largest Int elements",
                            node,
                        ),
                    },
                }
            }
            Intrinsic::RANGE_CONTAINS if kinds(&[K::RANGE, K::INTEGER]) => {
                let (range, item) = ((*arguments[0]).payload.range, (*arguments[1]).payload.integer);
                success(boolean_value(range.start <= item && item < range.end))
            }
            Intrinsic::FLOAT_FROM_INT if kinds(&[K::INTEGER]) => success(float((*arguments[0]).payload.integer as f64)),
            Intrinsic::FLOAT_TRUNCATE..=Intrinsic::FLOAT_ROUND if kinds(&[K::FLOAT]) => {
                let value = (*arguments[0]).payload.floating;
                success(float(match intrinsic {
                    Intrinsic::FLOAT_TRUNCATE => trunc(value),
                    Intrinsic::FLOAT_FLOOR => floor(value),
                    Intrinsic::FLOAT_CEILING => ceil(value),
                    // Half away from zero.
                    _ => round(value),
                }))
            }
            Intrinsic::FLOAT_TO_INT if kinds(&[K::FLOAT]) => {
                let value = (*arguments[0]).payload.floating;
                if value != trunc(value) {
                    return failure(constant(StandardConstructor::FLOAT_NOT_INTEGER));
                }
                // -2^63 is exactly representable; 2^63 is the first value above Int.
                if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&value) {
                    return failure(constant(StandardConstructor::FLOAT_OUT_OF_RANGE));
                }
                success(int(value as i64))
            }
            Intrinsic::DEBUG_RENDER | Intrinsic::DEBUG_RENDER_INDENTED if arguments.len() == 1 => {
                success(render(arguments[0], intrinsic == Intrinsic::DEBUG_RENDER_INDENTED))
            }
            Intrinsic::FLOAT_TO_TEXT if kinds(&[K::FLOAT]) => {
                success(new_text(float::text((*arguments[0]).payload.floating).as_bytes()))
            }
            Intrinsic::FLOAT_PARSE if kinds(&[K::TEXT]) => match parse_float(arguments[0]) {
                Ok(value) => success(float(value)),
                Err(constructor) => failure(constant(constructor)),
            },
            _ => shape_defect(call_node),
        }
    }
}
