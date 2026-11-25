//! The C view of [`crate::decimal`], declared again in `runtime.c` as
//! `ChimiDecimal`, the layout of a `Decimal` value's payload. The version
//! suffix changes with the layout, so a stale archive fails to link instead
//! of misreading memory.
//!
//! Every function takes decimals the runtime built through these functions,
//! so an out-of-range one is a runtime bug and aborts the program.

use crate::decimal::{self, Decimal, Rounding};
use core::cmp::Ordering;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ChimiDecimal {
    pub high: u64,
    pub low: u64,
    pub scale: u32,
    pub negative: bool,
}

impl From<Decimal> for ChimiDecimal {
    fn from(value: Decimal) -> Self {
        let (negative, magnitude, scale) = value.parts();
        Self {
            high: (magnitude >> 64) as u64,
            low: magnitude as u64,
            scale,
            negative,
        }
    }
}

fn decimal(value: ChimiDecimal) -> Decimal {
    let magnitude = (u128::from(value.high) << 64) | u128::from(value.low);
    match Decimal::new(value.negative, magnitude, value.scale) {
        Some(decimal) => decimal,
        None => panic!(),
    }
}

/// Writes `result` to `out` when there is one, and returns whether there was.
///
/// # Safety
///
/// `out` must point to writable memory for a `ChimiDecimal`.
unsafe fn write(result: Option<Decimal>, out: *mut ChimiDecimal) -> bool {
    let Some(value) = result else { return false };
    // SAFETY: the caller guarantees `out` is writable.
    unsafe { out.write(value.into()) };
    true
}

/// The decimal `magnitude × 10^-scale` (negative when `negative`), or false
/// when the magnitude has more than 38 digits or the scale exceeds 38.
///
/// # Safety
///
/// `out` must point to writable memory for a `ChimiDecimal`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_decimal_new_v1(
    negative: bool,
    high: u64,
    low: u64,
    scale: u32,
    out: *mut ChimiDecimal,
) -> bool {
    let magnitude = (u128::from(high) << 64) | u128::from(low);
    // SAFETY: forwarded from the caller.
    unsafe { write(Decimal::new(negative, magnitude, scale), out) }
}

#[unsafe(no_mangle)]
pub extern "C" fn chimi_decimal_negate_v1(value: ChimiDecimal) -> ChimiDecimal {
    decimal(value).negate().into()
}

/// # Safety
///
/// `out` must point to writable memory for a `ChimiDecimal`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_decimal_add_v1(
    left: ChimiDecimal,
    right: ChimiDecimal,
    out: *mut ChimiDecimal,
) -> bool {
    // SAFETY: forwarded from the caller.
    unsafe { write(decimal(left).add(decimal(right)), out) }
}

/// # Safety
///
/// `out` must point to writable memory for a `ChimiDecimal`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_decimal_subtract_v1(
    left: ChimiDecimal,
    right: ChimiDecimal,
    out: *mut ChimiDecimal,
) -> bool {
    // SAFETY: forwarded from the caller.
    unsafe { write(decimal(left).subtract(decimal(right)), out) }
}

/// # Safety
///
/// `out` must point to writable memory for a `ChimiDecimal`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_decimal_multiply_v1(
    left: ChimiDecimal,
    right: ChimiDecimal,
    out: *mut ChimiDecimal,
) -> bool {
    // SAFETY: forwarded from the caller.
    unsafe { write(decimal(left).multiply(decimal(right)), out) }
}

/// `dividend / divisor` with `scale` fraction digits, rounded by the
/// `std/decimal.rounding_code` `rounding`; false for a zero divisor, an
/// out-of-range quotient, a scale above 38 or an unknown rounding code.
///
/// # Safety
///
/// `out` must point to writable memory for a `ChimiDecimal`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_decimal_divide_v1(
    dividend: ChimiDecimal,
    divisor: ChimiDecimal,
    scale: u32,
    rounding: u32,
    out: *mut ChimiDecimal,
) -> bool {
    let Some(rounding) = Rounding::from_code(i64::from(rounding)) else {
        return false;
    };
    if scale > decimal::MAX_SCALE {
        return false;
    }
    // SAFETY: forwarded from the caller.
    unsafe {
        write(
            decimal(dividend).divide(decimal(divisor), scale, rounding),
            out,
        )
    }
}

/// Numeric order, so 1.50 and 1.5 are equal: -1, 0 or 1.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_decimal_compare_v1(left: ChimiDecimal, right: ChimiDecimal) -> i32 {
    match decimal(left).numeric_cmp(decimal(right)) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// Writes the `Decimal.to_text` form, or with `normalized` the debug form
/// without trailing fraction zeros, as a NUL-terminated string, and returns
/// its length.
///
/// # Safety
///
/// `out` must point to `decimal::TEXT_BYTES + 1` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_decimal_text_v1(
    value: ChimiDecimal,
    normalized: bool,
    out: *mut u8,
) -> usize {
    let value = decimal(value);
    let text = if normalized { value.normalized() } else { value }.text();
    let bytes = text.as_bytes();
    // SAFETY: the caller guarantees room for TEXT_BYTES bytes and a NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        out.add(bytes.len()).write(0);
    }
    bytes.len()
}

/// Reads `length` bytes at `text` as `Decimal.read` does.
///
/// # Safety
///
/// `text` must point to `length` readable bytes (it may be null when `length`
/// is 0), and `out` to writable memory for a `ChimiDecimal`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_decimal_parse_v1(
    text: *const u8,
    length: usize,
    out: *mut ChimiDecimal,
) -> bool {
    let text: &[u8] = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `length` readable bytes.
        unsafe { core::slice::from_raw_parts(text, length) }
    };
    // SAFETY: forwarded from the caller.
    unsafe { write(Decimal::parse_bytes(text), out) }
}
