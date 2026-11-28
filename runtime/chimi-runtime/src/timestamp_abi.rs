//! The C view of [`crate::timestamp`].

use crate::timestamp;

/// Writes the RFC 3339 text of `nanoseconds` as a NUL-terminated string and
/// returns its length.
///
/// # Safety
///
/// `out` must point to `timestamp::TEXT_BYTES + 1` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_timestamp_text_v1(nanoseconds: i64, out: *mut u8) -> usize {
    let text = timestamp::text(nanoseconds);
    let bytes = text.as_bytes();
    // SAFETY: the caller guarantees room for TEXT_BYTES bytes and a NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        out.add(bytes.len()).write(0);
    }
    bytes.len()
}

/// Parses the `length` bytes at `text` as an RFC 3339 date-time into
/// `*nanoseconds`; false, leaving it alone, when they are not one.
///
/// # Safety
///
/// `text` points to `length` readable bytes (or `length` is 0), and
/// `nanoseconds` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_timestamp_parse_v1(text: *const u8, length: usize, nanoseconds: *mut i64) -> bool {
    let bytes = if length == 0 {
        &[][..]
    } else {
        // SAFETY: the caller passes `length` readable bytes.
        unsafe { core::slice::from_raw_parts(text, length) }
    };
    let Some(parsed) = timestamp::parse(bytes) else { return false };
    // SAFETY: the caller passes a writable output.
    unsafe { nanoseconds.write(parsed) };
    true
}
