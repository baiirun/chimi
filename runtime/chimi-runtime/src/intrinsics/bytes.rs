//! The native core of std/bytes, and std/http's byte scan;
//! `execution/bytes_operations.rs` is the interpreter's, and the defect
//! messages match it. A `Bytes` shares Text's payload with its own kind, so
//! the collector, heap copies and transfers treat its buffer as a Text's.
//! Every value is allocated in the order `runtime.c` allocated it, so memory
//! reports do not change.

use core::ffi::c_void;
use core::ptr;

use super::message::Message;
use super::standard::{defect, defect_with, option_value, shape_defect, success};
use super::text::HeapText;
use crate::heap::allocator::allocate;
use crate::heap::layout::{AllocationKind, Control, Field, Intrinsic, Value, ValueKind};
use crate::heap::list;
use crate::heap::values::{allocate_kind, integer_value, new_value, text_value};

/// The bytes of a Text or Bytes value.
///
/// # Safety
/// `value` is a live Text or Bytes.
unsafe fn bytes_of<'a>(value: *const Value) -> &'a [u8] {
    // SAFETY: the caller's contract; the buffer is never null.
    unsafe {
        let payload = (*value).payload.text;
        core::slice::from_raw_parts(payload.bytes.cast::<u8>(), payload.length)
    }
}

/// A Bytes holding a copy of `bytes`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn bytes_value(bytes: &[u8]) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = text_value(bytes.as_ptr().cast(), bytes.len());
        (*value).kind = ValueKind::BYTES;
        value
    }
}

/// An Int.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn int(raw: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::INTEGER, raw) }
}

/// `[start, end)` clamped to `[0, length]`; a reversed range clamps to empty.
fn clamp_range(start: i64, end: i64, length: usize) -> (usize, usize) {
    let clamp = |bound: i64| usize::try_from(bound).map_or(0, |bound| bound.min(length));
    let low = clamp(start);
    (low, clamp(end).max(low))
}

/// The UTF-8 sequence at the start of `text` (which is not empty): how many
/// bytes it spans, and whether they form a whole scalar. An incomplete one is
/// the maximal subpart Unicode recommends replacing with one U+FFFD (at least
/// one byte), which is what Rust's `from_utf8_lossy` replaces. Ranges follow
/// Unicode Table 3-7, so overlong forms, surrogates, and values past
/// U+10FFFF fail.
fn utf8_sequence(text: &[u8]) -> (usize, bool) {
    let first = text[0];
    let (mut low, mut high) = (0x80, 0xbf);
    let width = match first {
        0..=0x7f => return (1, true),
        0xc2..=0xdf => 2,
        0xe0..=0xef => {
            if first == 0xe0 {
                low = 0xa0;
            }
            if first == 0xed {
                high = 0x9f;
            }
            3
        }
        0xf0..=0xf4 => {
            if first == 0xf0 {
                low = 0x90;
            }
            if first == 0xf4 {
                high = 0x8f;
            }
            4
        }
        _ => return (1, false),
    };
    for taken in 1..width {
        match text.get(taken) {
            Some(&byte) if (low..=high).contains(&byte) => {}
            _ => return (taken, false),
        }
        (low, high) = (0x80, 0xbf);
    }
    (width, true)
}

/// An integer layout of std/bytes' `layout_code`: its width in bytes,
/// whether it is signed, and whether it is big-endian.
struct IntLayout {
    width: usize,
    signed: bool,
    big_endian: bool,
}

/// `layout_code`: U8, I8, then U16Be, U16Le, I16Be, I16Le and the same four
/// for 32 bits, then I64Be, I64Le.
fn int_layout(code: i64) -> Option<IntLayout> {
    Some(match code {
        0 | 1 => IntLayout { width: 1, signed: code == 1, big_endian: true },
        2..=9 => {
            let within = code - 2;
            IntLayout { width: if within < 4 { 2 } else { 4 }, signed: within % 4 >= 2, big_endian: within % 2 == 0 }
        }
        10 | 11 => IntLayout { width: 8, signed: true, big_endian: code == 10 },
        _ => return None,
    })
}

/// std/bytes' (or std/http's byte scan) intrinsic `intrinsic` on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn bytes_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let kinds = |kinds: &[ValueKind]| {
            arguments.len() == kinds.len() && arguments.iter().zip(kinds).all(|(&value, &kind)| (*value).kind == kind)
        };
        let integer = |index: usize| (*arguments[index]).payload.integer;
        let node = i64::from(call_node);
        use ValueKind as K;
        match intrinsic {
            Intrinsic::BYTES_FROM if kinds(&[K::LIST]) => {
                let items = arguments[0];
                let length = (*items).payload.list.length;
                let raw = allocate(length as usize + 1, 1);
                for index in 0..length {
                    let element = list::get(items, index);
                    if (*element).kind != K::INTEGER {
                        return shape_defect(call_node);
                    }
                    let byte = (*element).payload.integer;
                    let Ok(byte) = u8::try_from(byte) else {
                        let message = Message::new()
                            .text(b"Bytes.from: element ")
                            .integer(i64::from(index))
                            .text(b" is ")
                            .integer(byte)
                            .text(b", outside 0..=255");
                        return defect_with(message.finish(), node);
                    };
                    *raw.add(index as usize) = byte;
                }
                *raw.add(length as usize) = 0;
                let value = new_value(K::BYTES);
                (*value).payload.text.bytes = raw.cast();
                (*value).payload.text.length = length as usize;
                success(value)
            }
            Intrinsic::BYTES_LENGTH if kinds(&[K::BYTES]) => success(int((*arguments[0]).payload.text.length as i64)),
            Intrinsic::BYTES_GET if kinds(&[K::BYTES, K::INTEGER]) => {
                let byte = usize::try_from(integer(1)).ok().and_then(|index| bytes_of(arguments[0]).get(index));
                option_value(byte.map_or(ptr::null_mut(), |&byte| int(i64::from(byte))))
            }
            Intrinsic::BYTES_INDEX_OF if kinds(&[K::BYTES, K::BYTES]) => {
                let (haystack, needle) = (bytes_of(arguments[0]), bytes_of(arguments[1]));
                if needle.is_empty() {
                    return defect(c"Bytes.index_of: needle must not be empty", node);
                }
                let found = libc::memmem(
                    haystack.as_ptr().cast::<c_void>(),
                    haystack.len(),
                    needle.as_ptr().cast::<c_void>(),
                    needle.len(),
                );
                option_value(if found.is_null() {
                    ptr::null_mut()
                } else {
                    int(found.cast::<u8>().offset_from(haystack.as_ptr()) as i64)
                })
            }
            Intrinsic::BYTES_CONCAT if kinds(&[K::LIST]) => {
                let parts = arguments[0];
                let mut output = HeapText::new();
                for index in 0..(*parts).payload.list.length {
                    let part = list::get(parts, index);
                    if (*part).kind != K::BYTES {
                        return shape_defect(call_node);
                    }
                    output.push_bytes(bytes_of(part));
                }
                success(bytes_value(output.bytes()))
            }
            Intrinsic::BYTES_SLICE if kinds(&[K::BYTES, K::RANGE]) => {
                let bytes = bytes_of(arguments[0]);
                let range = (*arguments[1]).payload.range;
                let (from, to) = clamp_range(range.start, range.end, bytes.len());
                success(bytes_value(&bytes[from..to]))
            }
            Intrinsic::BYTES_TO_LIST if kinds(&[K::BYTES]) => {
                let bytes = bytes_of(arguments[0]);
                let items = allocate(bytes.len().max(1), size_of::<*mut Value>()).cast::<*mut Value>();
                for (index, &byte) in bytes.iter().enumerate() {
                    *items.add(index) = int(i64::from(byte));
                }
                let length = bytes.len() as u32;
                success(list::list_value(list::from_values(items, length), 0, length))
            }
            Intrinsic::BYTES_FROM_TEXT if kinds(&[K::TEXT]) => success(bytes_value(bytes_of(arguments[0]))),
            Intrinsic::BYTES_TO_TEXT_LOSSY if kinds(&[K::BYTES]) => {
                let bytes = bytes_of(arguments[0]);
                let mut output = HeapText::new();
                let mut at = 0;
                while at < bytes.len() {
                    let (width, complete) = utf8_sequence(&bytes[at..]);
                    output.push_bytes(if complete { &bytes[at..at + width] } else { "\u{fffd}".as_bytes() });
                    at += width;
                }
                let text = output.bytes();
                success(text_value(text.as_ptr().cast(), text.len()))
            }
            Intrinsic::HTTP_COUNT_ENDS if kinds(&[K::BYTES, K::BYTES]) => {
                let mut allowed = [false; 256];
                for &byte in bytes_of(arguments[1]) {
                    allowed[usize::from(byte)] = true;
                }
                let bytes = bytes_of(arguments[0]);
                let leading = bytes.iter().take_while(|&&byte| allowed[usize::from(byte)]).count();
                let trailing = if leading == bytes.len() {
                    bytes.len()
                } else {
                    bytes.iter().rev().take_while(|&&byte| allowed[usize::from(byte)]).count()
                };
                let pair = new_value(K::TUPLE);
                (*pair).payload.record.length = 2;
                let fields = allocate_kind::<Field>(2, AllocationKind::FIELDS);
                (*pair).payload.record.fields = fields;
                *fields = Field { name: ptr::null(), value: int(leading as i64) };
                *fields.add(1) = Field { name: ptr::null(), value: int(trailing as i64) };
                success(pair)
            }
            Intrinsic::BYTES_VALID_UTF8_LENGTH if kinds(&[K::BYTES]) => {
                let bytes = bytes_of(arguments[0]);
                let valid = core::str::from_utf8(bytes).map_or_else(|error| error.valid_up_to(), |_| bytes.len());
                success(int(valid as i64))
            }
            Intrinsic::BYTES_READ_INT_CODE if kinds(&[K::BYTES, K::INTEGER, K::INTEGER]) => {
                let Some(layout) = int_layout(integer(2)) else { return shape_defect(call_node) };
                let bytes = bytes_of(arguments[0]);
                let field = usize::try_from(integer(1))
                    .ok()
                    .and_then(|at| bytes.get(at..))
                    .and_then(|rest| rest.get(..layout.width));
                let Some(field) = field else { return option_value(ptr::null_mut()) };
                let mut raw = 0u64;
                for index in 0..layout.width {
                    raw = raw << 8 | u64::from(field[if layout.big_endian { index } else { layout.width - 1 - index }]);
                }
                // Sign-extend from the layout's width; 64 bits already is.
                let bits = 8 * layout.width;
                let value = if layout.signed && bits < 64 && (raw >> (bits - 1)) & 1 == 1 {
                    (raw | (u64::MAX << bits)) as i64
                } else {
                    raw as i64
                };
                option_value(int(value))
            }
            Intrinsic::BYTES_INT_BYTES if kinds(&[K::INTEGER, K::INTEGER]) => {
                let Some(layout) = int_layout(integer(1)) else { return shape_defect(call_node) };
                let value = integer(0);
                if layout.width < 8 {
                    let bits = 8 * layout.width as u32;
                    let (low, high) = if layout.signed {
                        (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
                    } else {
                        (0, (1i64 << bits) - 1)
                    };
                    if value < low || value > high {
                        return option_value(ptr::null_mut());
                    }
                }
                let mut raw = [0u8; 8];
                for index in 0..layout.width {
                    let byte = ((value as u64) >> (8 * (layout.width - 1 - index))) as u8;
                    raw[if layout.big_endian { index } else { layout.width - 1 - index }] = byte;
                }
                option_value(bytes_value(&raw[..layout.width]))
            }
            // EMBED_TEXT and EMBED_BYTES never run here: lowering admits them
            // only in module constants, which the build evaluates before
            // emitting program data.
            _ => shape_defect(call_node),
        }
    }
}
