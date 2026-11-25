//! The operating-system calls the binding packages share: `errno`, and the
//! C library's text for an error.

use core::ffi::{c_char, c_int};

/// The calling thread's `errno`.
pub fn errno() -> c_int {
    // SAFETY: the C library's errno location is always valid for the thread.
    unsafe { *errno_location() }
}

/// The C library's message for `error`, valid until the next `strerror` on
/// this thread.
pub fn error_text(error: c_int) -> *const c_char {
    // SAFETY: strerror returns a NUL-terminated string for any code.
    unsafe { libc::strerror(error) }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe fn errno_location() -> *mut c_int {
    // SAFETY: glibc and musl define it for every thread.
    unsafe { libc::__errno_location() }
}

#[cfg(target_vendor = "apple")]
unsafe fn errno_location() -> *mut c_int {
    // SAFETY: libSystem defines it for every thread.
    unsafe { libc::__error() }
}

/// `value`'s decimal digits followed by a NUL, in `out`, which holds any
/// `u16`; returns the digits without the NUL.
// A u16 has at most 5 digits, so `start` stays in 0..5.
#[allow(clippy::indexing_slicing)]
pub fn decimal(value: u16, out: &mut [u8; 6]) -> &[u8] {
    let mut start = out.len() - 1;
    let mut rest = value;
    out[start] = 0;
    loop {
        start -= 1;
        out[start] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    out.copy_within(start.., 0);
    &out[..out.len() - 1 - start]
}
