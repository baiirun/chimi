//! The C view of [`crate::float`]'s text form.

use crate::float;

/// Writes the canonical text of the finite `value` as a NUL-terminated string
/// and returns its length.
///
/// # Safety
///
/// `out` must point to `float::TEXT_BYTES + 1` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_float_text_v1(value: f64, out: *mut u8) -> usize {
    let text = float::text(value);
    let bytes = text.as_bytes();
    // SAFETY: the caller guarantees room for TEXT_BYTES bytes and a NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        out.add(bytes.len()).write(0);
    }
    bytes.len()
}
