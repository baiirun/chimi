//! The C view of [`crate::text`] and [`crate::graphemes`]. Every function
//! takes a `Text` value's bytes, which the runtime keeps valid UTF-8; bytes
//! that are not are a runtime bug and abort the program.

use crate::{graphemes, text};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

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

/// Where a trim leaves the text: `bytes[start..start + length]`.
#[repr(C)]
pub struct ChimiTextSpan {
    pub start: usize,
    pub length: usize,
}

/// `Text.trim_start`, or with `end` `Text.trim_end`, as the span of the
/// input that remains.
///
/// # Safety
///
/// `bytes` must point to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_text_trim_v1(bytes: *const u8, length: usize, end: bool) -> ChimiTextSpan {
    // SAFETY: forwarded from the caller.
    let whole = unsafe { utf8(bytes, length) };
    let kept = if end { text::trim_end(whole) } else { text::trim_start(whole) };
    ChimiTextSpan {
        start: kept.as_ptr() as usize - whole.as_ptr() as usize,
        length: kept.len(),
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

/// `Text.uppercase` when `upper`, otherwise `Text.lowercase`.
///
/// # Safety
///
/// `bytes` must point to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_text_case_map_v1(bytes: *const u8, length: usize, upper: bool) -> ChimiOwnedText {
    // SAFETY: forwarded from the caller.
    let source = unsafe { utf8(bytes, length) };
    owned_text(if upper { text::uppercase(source) } else { text::lowercase(source) })
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
/// `owned` must come from [`chimi_text_case_map_v1`] or
/// `chimi_metrics_document_v1` and not have been released already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_text_release_v1(owned: ChimiOwnedText) {
    // SAFETY: the caller passes back the box `owned_text` made.
    drop(unsafe { Box::from_raw(owned.owned) });
}

/// The end offsets of a text's grapheme clusters, in order, for the C
/// runtime to read and then release with [`chimi_text_release_ends_v1`].
#[repr(C)]
pub struct ChimiGraphemeEnds {
    pub ends: *const usize,
    pub count: usize,
    owned: *mut Vec<usize>,
}

/// `Text.graphemes`, as the byte offset where each cluster ends; the text is
/// read once, so this is linear in its length.
///
/// # Safety
///
/// `bytes` must point to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_text_grapheme_ends_v1(bytes: *const u8, length: usize) -> ChimiGraphemeEnds {
    // SAFETY: forwarded from the caller.
    let whole = unsafe { utf8(bytes, length) };
    let mut ends = Box::new(Vec::new());
    let mut start = 0;
    while start < whole.len() {
        start = graphemes::grapheme_end(whole, start);
        ends.push(start);
    }
    ChimiGraphemeEnds {
        ends: ends.as_ptr(),
        count: ends.len(),
        owned: Box::into_raw(ends),
    }
}

/// # Safety
///
/// `owned` must come from [`chimi_text_grapheme_ends_v1`] and not have been
/// released already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_text_release_ends_v1(owned: ChimiGraphemeEnds) {
    // SAFETY: the caller passes back the box `chimi_text_grapheme_ends_v1` made.
    drop(unsafe { Box::from_raw(owned.owned) });
}
