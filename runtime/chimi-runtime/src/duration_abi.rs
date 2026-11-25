//! The C view of [`crate::duration`]'s text form and parser. A `Duration`
//! crosses as its `int64_t` count of nanoseconds.

use crate::duration::{self, ParseFailure};

/// Writes the canonical text form as a NUL-terminated string and returns its
/// length.
///
/// # Safety
///
/// `out` must point to `duration::TEXT_BYTES + 1` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_duration_text_v1(nanoseconds: i64, out: *mut u8) -> usize {
    let text = duration::text(nanoseconds);
    let bytes = text.as_bytes();
    // SAFETY: the caller guarantees room for TEXT_BYTES bytes and a NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        out.add(bytes.len()).write(0);
    }
    bytes.len()
}

/// `Duration.parse` over `length` bytes at `text`: 0 with `out` set, or 1
/// Empty, 2 Invalid, 3 OutOfRange. Bytes that are not UTF-8 are Invalid.
///
/// # Safety
///
/// `text` must point to `length` readable bytes (it may be null when `length`
/// is 0), and `out` to a writable `int64_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_duration_parse_v1(
    text: *const u8,
    length: usize,
    out: *mut i64,
) -> u32 {
    let bytes: &[u8] = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `length` readable bytes.
        unsafe { core::slice::from_raw_parts(text, length) }
    };
    let Ok(text) = core::str::from_utf8(bytes) else {
        return 2;
    };
    match duration::parse(text) {
        Ok(nanoseconds) => {
            // SAFETY: the caller guarantees `out` is writable.
            unsafe { out.write(nanoseconds) };
            0
        }
        Err(ParseFailure::Empty) => 1,
        Err(ParseFailure::Invalid) => 2,
        Err(ParseFailure::OutOfRange) => 3,
    }
}
