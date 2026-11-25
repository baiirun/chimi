//! The C view of [`crate::json`], declared again in `runtime.c` as
//! `ChimiJsonParse` and `ChimiJsonNode`. The version suffix changes with the
//! layout, so a stale archive fails to link instead of misreading memory.

use crate::json::{self, Node};
use alloc::borrow::Cow;
use alloc::boxed::Box;
use core::ffi::c_void;

/// What a parse leaves for the C runtime to read. On success `nodes` holds
/// the tape and `error_message` is null; on failure `node_count` is 0 and
/// the message and offset describe the error. Every pointer stays valid
/// until [`chimi_json_release_v1`].
#[repr(C)]
pub struct ChimiJsonParse {
    pub nodes: *const Node,
    pub node_count: usize,
    pub decoded: *const u8,
    pub error_message: *const u8,
    pub error_message_length: usize,
    pub error_offset: usize,
    owned: *mut c_void,
}

/// Kept only to be dropped: the parse's pointers point into it.
#[allow(dead_code)]
enum Owned {
    Tape(json::Tape),
    Error(Cow<'static, [u8]>),
}

const _: () = assert!(core::mem::size_of::<Node>() == 3 * core::mem::size_of::<usize>());

/// Decodes `length` bytes at `source` into `out`, and returns whether they
/// held one JSON value.
///
/// # Safety
///
/// `source` must point to `length` readable bytes (it may be null when
/// `length` is 0), and `out` to writable memory for a `ChimiJsonParse`,
/// which the caller passes to [`chimi_json_release_v1`] once read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_json_parse_v1(
    source: *const u8,
    length: usize,
    out: *mut ChimiJsonParse,
) -> bool {
    let source: &[u8] = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `length` readable bytes.
        unsafe { core::slice::from_raw_parts(source, length) }
    };
    // A `Vec`'s buffer does not move when the `Vec` moves into the box, so
    // the pointers taken first stay valid until the box is dropped.
    let (parse, succeeded) = match json::decode(source) {
        Ok(tape) => {
            let parse = ChimiJsonParse {
                nodes: tape.nodes.as_ptr(),
                node_count: tape.nodes.len(),
                decoded: tape.decoded.as_ptr(),
                error_message: core::ptr::null(),
                error_message_length: 0,
                error_offset: 0,
                owned: Box::into_raw(Box::new(Owned::Tape(tape))).cast(),
            };
            (parse, true)
        }
        Err(error) => {
            let parse = ChimiJsonParse {
                nodes: core::ptr::null(),
                node_count: 0,
                decoded: core::ptr::null(),
                error_message: error.message.as_ptr(),
                error_message_length: error.message.len(),
                error_offset: error.offset,
                owned: Box::into_raw(Box::new(Owned::Error(error.message))).cast(),
            };
            (parse, false)
        }
    };
    // SAFETY: the caller guarantees `out` is writable.
    unsafe { out.write(parse) };
    succeeded
}

/// Frees what [`chimi_json_parse_v1`] left in `parse`, which must not be
/// read afterwards.
///
/// # Safety
///
/// `parse` must have been filled by `chimi_json_parse_v1` and not released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_json_release_v1(parse: *mut ChimiJsonParse) {
    // SAFETY: the caller guarantees `parse` was filled by a parse, whose
    // `owned` came from `Box::into_raw` of an `Owned`.
    unsafe {
        let owned = core::mem::replace(&mut (*parse).owned, core::ptr::null_mut());
        if !owned.is_null() {
            drop(Box::from_raw(owned.cast::<Owned>()));
        }
    }
}
