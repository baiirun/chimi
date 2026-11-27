//! The C view of [`crate::duration`]'s text form. A `Duration`
//! crosses as its `int64_t` count of nanoseconds.

use crate::duration;

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
