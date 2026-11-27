//! `Debug.crash`, `Option.unwrap_or_fail`, and the native cores of std/text
//! and std/list; `execution/core_operations.rs` is the interpreter's, and the
//! defect messages match it. Every value is allocated in the order
//! `runtime.c` allocated it, so memory reports do not change.

use core::ffi::c_char;
use core::ptr;

use super::message::Message;
use super::standard::{boolean_value, defect, defect_with, failure, option_value, shape_defect, success};
use super::text::HeapText;
use crate::heap::allocator::current_carrier;
use crate::heap::layout::{Control, Intrinsic, ListVector, Value, ValueKind};
use crate::heap::list;
use crate::heap::values::{integer_value, text_value};
use crate::text_abi::utf8;
use crate::{graphemes, text};

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// The bytes of a Text value.
///
/// # Safety
/// `value` is a live Text.
unsafe fn bytes_of<'a>(value: *const Value) -> &'a [u8] {
    // SAFETY: the caller's contract; the buffer is never null.
    unsafe {
        let payload = (*value).payload.text;
        core::slice::from_raw_parts(payload.bytes.cast::<u8>(), payload.length)
    }
}

/// A Text value's bytes as text; the runtime keeps them UTF-8.
///
/// # Safety
/// `value` is a live Text.
unsafe fn str_of<'a>(value: *const Value) -> &'a str {
    // SAFETY: the caller's contract.
    unsafe {
        let payload = (*value).payload.text;
        utf8(payload.bytes.cast(), payload.length)
    }
}

/// A Text holding a copy of `bytes`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn new_text(bytes: &[u8]) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { text_value(bytes.as_ptr().cast(), bytes.len()) }
}

/// `[start, end)` clamped to `[0, length]`; a reversed range clamps to empty.
fn clamp_range(start: i64, end: i64, length: usize) -> (usize, usize) {
    let clamp = |bound: i64| usize::try_from(bound).map_or(0, |bound| bound.min(length));
    let low = clamp(start);
    (low, clamp(end).max(low))
}

/// Whether `offset` starts a UTF-8 sequence of `text` or is its end.
fn utf8_boundary(text: &[u8], offset: usize) -> bool {
    offset == text.len() || text[offset] & 0xc0 != 0x80
}

/// Where `needle` (not empty) first appears in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    // SAFETY: both slices are readable for their lengths.
    let found = unsafe { libc::memmem(haystack.as_ptr().cast(), haystack.len(), needle.as_ptr().cast(), needle.len()) };
    // SAFETY: a found needle lies inside `haystack`.
    (!found.is_null()).then(|| unsafe { found.cast::<u8>().offset_from(haystack.as_ptr()) } as usize)
}

/// A copy of `message` up to its first U+0000, outside every heap: a
/// control's message is not traced, and a loop region or a child task's heap
/// may be freed while the defect it ended with is still read.
pub(super) fn lasting_message(message: &[u8]) -> *const c_char {
    let length = message.iter().position(|&byte| byte == 0).unwrap_or(message.len());
    // SAFETY: a plain allocation; failure exits.
    let copy = unsafe { libc::malloc(length + 1) }.cast::<u8>();
    if copy.is_null() {
        // SAFETY: exits.
        unsafe { chimi_runtime_exhausted_memory() }
    }
    // SAFETY: the block holds `length` bytes and the NUL.
    unsafe {
        ptr::copy_nonoverlapping(message.as_ptr(), copy, length);
        *copy.add(length) = 0;
    }
    copy.cast()
}

/// An empty List.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn empty_list() -> *mut Value {
    let vector = ListVector { root: ptr::null_mut(), tail: ptr::null_mut(), length: 0 };
    // SAFETY: the caller's contract.
    unsafe { list::list_value(vector, 0, 0) }
}

/// The intrinsic `intrinsic` of this family on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn core_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    use ValueKind as K;
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let kinds = |expected: &[ValueKind]| {
            arguments.len() == expected.len() && arguments.iter().zip(expected).all(|(&value, &kind)| (*value).kind == kind)
        };
        let texts = |count: usize| arguments.len() == count && arguments.iter().all(|&value| (*value).kind == K::TEXT);
        match intrinsic {
            Intrinsic::DEBUG_CRASH if kinds(&[K::TEXT]) => defect_with(lasting_message(bytes_of(arguments[0])), node),
            Intrinsic::OPTION_UNWRAP_OR_FAIL if arguments.len() == 2 && (*arguments[0]).kind == K::CONSTRUCTED => {
                let option = (*arguments[0]).payload.constructed;
                if option.length == 1 { success((*option.fields).value) } else { failure(arguments[1]) }
            }
            Intrinsic::TEXT_CONCAT if kinds(&[K::LIST]) => {
                let parts = arguments[0];
                let mut output = HeapText::new();
                for index in 0..(*parts).payload.list.length {
                    let part = list::get(parts, index);
                    if (*part).kind != K::TEXT {
                        return shape_defect(call_node);
                    }
                    output.push_bytes(bytes_of(part));
                }
                success(new_text(output.bytes()))
            }
            Intrinsic::TEXT_BYTE_LENGTH if kinds(&[K::TEXT]) => {
                success(integer_value(K::INTEGER, (*arguments[0]).payload.text.length as i64))
            }
            Intrinsic::TEXT_BYTE_SLICE if kinds(&[K::TEXT, K::RANGE]) => {
                let bytes = bytes_of(arguments[0]);
                let range = (*arguments[1]).payload.range;
                let (from, to) = clamp_range(range.start, range.end, bytes.len());
                for bound in [from, to] {
                    if !utf8_boundary(bytes, bound) {
                        let message = Message::new()
                            .text(b"Text.byte_slice: bound ")
                            .integer(bound as i64)
                            .text(b" falls inside a UTF-8 sequence");
                        return defect_with(message.finish(), node);
                    }
                }
                success(new_text(&bytes[from..to]))
            }
            Intrinsic::TEXT_CONTAINS if texts(2) => {
                let needle = bytes_of(arguments[1]);
                if needle.is_empty() {
                    return defect(c"Text.contains: needle must not be empty", node);
                }
                success(boolean_value(find(bytes_of(arguments[0]), needle).is_some()))
            }
            Intrinsic::TEXT_STARTS_WITH if texts(2) => {
                success(boolean_value(bytes_of(arguments[0]).starts_with(bytes_of(arguments[1]))))
            }
            Intrinsic::TEXT_ENDS_WITH if texts(2) => {
                success(boolean_value(bytes_of(arguments[0]).ends_with(bytes_of(arguments[1]))))
            }
            Intrinsic::TEXT_SPLIT if texts(2) => {
                let (mut rest, separator) = (bytes_of(arguments[0]), bytes_of(arguments[1]));
                if separator.is_empty() {
                    return defect(c"Text.split: separator must not be empty", node);
                }
                let mut parts = empty_list();
                while let Some(found) = find(rest, separator) {
                    parts = list::list_push(parts, new_text(&rest[..found]));
                    rest = &rest[found + separator.len()..];
                }
                success(list::list_push(parts, new_text(rest)))
            }
            Intrinsic::TEXT_REPLACE if texts(3) => {
                let (mut rest, pattern) = (bytes_of(arguments[0]), bytes_of(arguments[1]));
                let replacement = bytes_of(arguments[2]);
                if pattern.is_empty() {
                    return defect(c"Text.replace: pattern must not be empty", node);
                }
                let mut output = HeapText::new();
                while let Some(found) = find(rest, pattern) {
                    output.push_bytes(&rest[..found]);
                    output.push_bytes(replacement);
                    rest = &rest[found + pattern.len()..];
                }
                output.push_bytes(rest);
                success(new_text(output.bytes()))
            }
            Intrinsic::TEXT_TRIM_START | Intrinsic::TEXT_TRIM_END if kinds(&[K::TEXT]) => {
                let whole = str_of(arguments[0]);
                let kept = if intrinsic == Intrinsic::TEXT_TRIM_END { text::trim_end(whole) } else { text::trim_start(whole) };
                success(new_text(kept.as_bytes()))
            }
            Intrinsic::TEXT_LOWERCASE | Intrinsic::TEXT_UPPERCASE if kinds(&[K::TEXT]) => {
                let source = str_of(arguments[0]);
                let mapped = if intrinsic == Intrinsic::TEXT_UPPERCASE { text::uppercase(source) } else { text::lowercase(source) };
                success(new_text(mapped.as_bytes()))
            }
            Intrinsic::TEXT_GRAPHEMES if kinds(&[K::TEXT]) => {
                let whole = str_of(arguments[0]);
                let mut clusters = empty_list();
                for cluster in graphemes::graphemes(whole) {
                    clusters = list::list_push(clusters, new_text(cluster.as_bytes()));
                }
                success(clusters)
            }
            Intrinsic::LIST_GET if kinds(&[K::LIST, K::INTEGER]) => {
                let index = (*arguments[1]).payload.integer;
                let length = (*arguments[0]).payload.list.length;
                option_value(match u32::try_from(index) {
                    Ok(index) if index < length => list::get(arguments[0], index),
                    _ => ptr::null_mut(),
                })
            }
            Intrinsic::LIST_PUSH if arguments.len() == 2 && (*arguments[0]).kind == K::LIST => {
                success(list::list_push(arguments[0], arguments[1]))
            }
            Intrinsic::LIST_SLICE if kinds(&[K::LIST, K::RANGE]) => {
                let range = (*arguments[1]).payload.range;
                let (from, to) = clamp_range(range.start, range.end, (*arguments[0]).payload.list.length as usize);
                success(list::slice(current_carrier(), arguments[0], from as u32, to as u32))
            }
            Intrinsic::LIST_SET
                if arguments.len() == 3 && (*arguments[0]).kind == K::LIST && (*arguments[1]).kind == K::INTEGER =>
            {
                let window = (*arguments[0]).payload.list;
                let index = (*arguments[1]).payload.integer;
                match u32::try_from(index) {
                    Ok(index) if index < window.length => {
                        let vector = list::set(window.vector, window.start + index, arguments[2]);
                        success(list::list_value(vector, window.start, window.length))
                    }
                    _ => {
                        let message = Message::new()
                            .text(b"List.set: index must be in 0..")
                            .integer(i64::from(window.length))
                            .text(b", got ")
                            .integer(index);
                        defect_with(message.finish(), node)
                    }
                }
            }
            _ => shape_defect(call_node),
        }
    }
}
