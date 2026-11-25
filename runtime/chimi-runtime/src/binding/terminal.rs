//! std/terminal: whether a standard stream is a terminal, and its size.
//! Both are one non-blocking system call on the stream's descriptor, so
//! neither waits on the scheduler. A stream is its std/terminal.Standard
//! code: 0 input, 1 output, 2 error output.

use super::api::{Call, ChimiBindingCall, ChimiBindingResult};
use core::ffi::c_int;

/// The descriptor for the stream argument, or `None` for a code outside 0..2.
fn descriptor(call: &Call<'_>) -> Option<c_int> {
    call.integer(0)
        .filter(|code| (0..=2).contains(code))
        .map(|code| code as c_int)
}

fn is_terminal(fd: c_int) -> bool {
    // SAFETY: isatty only reads the descriptor table.
    unsafe { libc::isatty(fd) == 1 }
}

/// Columns and rows, or `None` when the stream is not a terminal or the
/// terminal reports no size (0 by 0, as serial consoles do).
fn size(fd: c_int) -> Option<(u16, u16)> {
    if !is_terminal(fd) {
        return None;
    }
    // SAFETY: an all-zero winsize is valid, and TIOCGWINSZ writes one.
    let mut size: libc::winsize = unsafe { core::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } != 0 {
        return None;
    }
    (size.ws_col != 0 && size.ws_row != 0).then_some((size.ws_col, size.ws_row))
}

/// `native_is_terminal(stream) -> Bool`
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_terminal_is_terminal(
    raw: *mut ChimiBindingCall,
) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let mut call = unsafe { Call::new(raw) };
    let Some(fd) = descriptor(&call) else {
        return call.defect(c"Terminal: not a standard stream").into_raw();
    };
    let value = call.make_boolean(is_terminal(fd));
    call.success(value).into_raw()
}

/// `native_size(stream) -> Option<(Int, Int)>`
///
/// # Safety
///
/// Only the runtime calls it, with the live binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_terminal_size(raw: *mut ChimiBindingCall) -> *mut ChimiBindingResult {
    // SAFETY: the runtime passes the call it is running.
    let mut call = unsafe { Call::new(raw) };
    let Some(fd) = descriptor(&call) else {
        return call.defect(c"Terminal: not a standard stream").into_raw();
    };
    let value = match size(fd) {
        None => call.none(),
        Some((columns, rows)) => {
            let items = [
                call.make_integer(i64::from(columns)),
                call.make_integer(i64::from(rows)),
            ];
            let tuple = call.make_tuple(&items);
            call.some(tuple)
        }
    };
    call.success(value).into_raw()
}
