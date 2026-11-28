//! Text crossing between Rust and `runtime.c`: a `Text` value's bytes read
//! as text, which the runtime keeps valid UTF-8 (bytes that are not are a
//! runtime bug and abort the program), and a text Rust built that C copies.

use alloc::boxed::Box;
use alloc::string::String;

/// # Safety
///
/// `bytes` must point to `length` readable bytes (it may be null when
/// `length` is 0) that outlive the returned reference.
pub(crate) unsafe fn utf8<'a>(bytes: *const u8, length: usize) -> &'a str {
    let bytes: &[u8] = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `length` readable bytes.
        unsafe { core::slice::from_raw_parts(bytes, length) }
    };
    match core::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => panic!(),
    }
}

/// A text the Rust side built, for the C runtime to copy and then release
/// with [`chimi_text_release_v1`].
#[repr(C)]
pub struct ChimiOwnedText {
    pub bytes: *const u8,
    pub length: usize,
    owned: *mut String,
}

/// Hands `text` to C until [`chimi_text_release_v1`].
pub(crate) fn owned_text(text: String) -> ChimiOwnedText {
    let text = Box::new(text);
    ChimiOwnedText {
        bytes: text.as_ptr(),
        length: text.len(),
        owned: Box::into_raw(text),
    }
}

/// # Safety
///
/// `owned` must come from `chimi_metrics_document_v1` or
/// `chimi_runtime_metrics_document_v1` and not have been
/// released already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_text_release_v1(owned: ChimiOwnedText) {
    // SAFETY: the caller passes back the box `owned_text` made.
    drop(unsafe { Box::from_raw(owned.owned) });
}
