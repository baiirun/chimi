//! The native core of std/decimal, over [`crate::decimal`];
//! `execution/decimal.rs` is the interpreter's. The Chimi wrappers check the
//! documented preconditions, so a broken one is a shape defect, as in the
//! interpreter.

use super::standard::{option_value, shape_defect, success};
use crate::decimal::{self, Decimal, Rounding};
use crate::heap::layout::{self, Control, Intrinsic, Value, ValueKind};
use crate::heap::values::{decimal_value, float_value, text_value};

/// The payload a Decimal value stores for `value`.
pub fn payload(value: Decimal) -> layout::Decimal {
    let (negative, magnitude, scale) = value.parts();
    layout::Decimal { high: (magnitude >> 64) as u64, low: magnitude as u64, scale, negative }
}

/// The decimal a Decimal value's payload stores. The runtime builds every
/// payload from a [`Decimal`], so one out of range is a runtime bug.
pub(super) fn of_payload(value: layout::Decimal) -> Decimal {
    let magnitude = (u128::from(value.high) << 64) | u128::from(value.low);
    match Decimal::new(value.negative, magnitude, value.scale) {
        Some(decimal) => decimal,
        None => panic!(),
    }
}

/// `Some` of a new Decimal value, or `None`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn optional(value: Option<Decimal>) -> *mut Control {
    // SAFETY: the caller's contract; Some's field is built first, as the C did.
    unsafe {
        match value {
            Some(value) => option_value(decimal_value(payload(value))),
            None => option_value(core::ptr::null_mut()),
        }
    }
}

unsafe extern "C" {
    /// The nearest double to the text, ties to even.
    fn strtod(text: *const core::ffi::c_char, end: *mut *mut core::ffi::c_char) -> f64;
}

/// std/decimal's intrinsic `intrinsic` on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn decimal_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let kinds = |kinds: &[ValueKind]| {
            arguments.len() == kinds.len() && arguments.iter().zip(kinds).all(|(&value, &kind)| (*value).kind == kind)
        };
        let integer = |index: usize| (*arguments[index]).payload.integer;
        let decimal = |index: usize| of_payload((*arguments[index]).payload.decimal);
        let scale = |index: usize| u32::try_from(integer(index)).ok().filter(|&scale| scale <= decimal::MAX_SCALE);
        const PAIR: &[ValueKind] = &[ValueKind::DECIMAL, ValueKind::DECIMAL];
        match intrinsic {
            Intrinsic::DECIMAL_SCALED if kinds(&[ValueKind::INTEGER, ValueKind::INTEGER]) => match scale(1) {
                Some(scale) => success(decimal_value(payload(Decimal::of(integer(0), scale)))),
                None => shape_defect(call_node),
            },
            Intrinsic::DECIMAL_READ if kinds(&[ValueKind::TEXT]) => {
                let text = (*arguments[0]).payload.text;
                let bytes = if text.length == 0 { &[][..] } else { core::slice::from_raw_parts(text.bytes.cast::<u8>(), text.length) };
                optional(Decimal::parse_bytes(bytes))
            }
            Intrinsic::DECIMAL_TO_TEXT if kinds(&[ValueKind::DECIMAL]) => {
                let text = decimal(0).text();
                let bytes = text.as_bytes();
                success(text_value(bytes.as_ptr().cast(), bytes.len()))
            }
            // strtod rounds the exact text correctly; Rust's own float
            // reading would add its code to every program.
            Intrinsic::DECIMAL_TO_FLOAT if kinds(&[ValueKind::DECIMAL]) => {
                let text = decimal(0).text();
                let bytes = text.as_bytes();
                let mut terminated = [0u8; decimal::TEXT_BYTES + 1];
                terminated[..bytes.len()].copy_from_slice(bytes);
                success(float_value(strtod(terminated.as_ptr().cast(), core::ptr::null_mut())))
            }
            Intrinsic::DECIMAL_ADD_CHECKED if kinds(PAIR) => optional(decimal(0).add(decimal(1))),
            Intrinsic::DECIMAL_SUBTRACT_CHECKED if kinds(PAIR) => optional(decimal(0).subtract(decimal(1))),
            Intrinsic::DECIMAL_MULTIPLY_CHECKED if kinds(PAIR) => optional(decimal(0).multiply(decimal(1))),
            Intrinsic::DECIMAL_DIVIDE_ROUNDED
                if kinds(&[ValueKind::DECIMAL, ValueKind::DECIMAL, ValueKind::INTEGER, ValueKind::INTEGER]) =>
            {
                match (scale(2), Rounding::from_code(integer(3))) {
                    (Some(scale), Some(rounding)) => optional(decimal(0).divide(decimal(1), scale, rounding)),
                    _ => shape_defect(call_node),
                }
            }
            _ => shape_defect(call_node),
        }
    }
}
