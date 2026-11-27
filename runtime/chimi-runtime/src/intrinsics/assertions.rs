//! The std/test assertions: `Test.assert_equal`, `Test.assert_not_equal`,
//! `Test.assert` and `Test.fail`; `execution/core_operations.rs` is the
//! interpreter's. A comparison's message is
//! "<heading>\nactual:\n<render>\nexpected:\n<render>", byte for byte as
//! `comparison_message` writes it, because `chimi test` reads the two
//! renderings back from it. Messages are built on the fiber's heap, in the
//! order `runtime.c` built them, so memory reports do not change.

use super::debug::render;
use super::standard::{defect_with, shape_defect, success, void_value};
use super::text::HeapText;
use crate::heap::compare::compare;
use crate::heap::layout::{Control, Intrinsic, Value, ValueKind};
use crate::heap::values::copy_bytes;

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

/// A defect whose message is `text` up to its first U+0000, copied onto the
/// heap, as `runtime.c`'s `copy_text` copied it.
///
/// # Safety
/// Runs on a fiber that may allocate; `text` is a live Text.
unsafe fn message_defect(text: *const Value, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let bytes = bytes_of(text);
        let length = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
        defect_with(copy_bytes(bytes.as_ptr().cast(), length), node)
    }
}

/// The intrinsic `intrinsic` of std/test's assertions on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn assertion_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let kind = |index: usize| (*arguments[index]).kind;
        match intrinsic {
            Intrinsic::TEST_ASSERT_EQUAL | Intrinsic::TEST_ASSERT_NOT_EQUAL if arguments.len() == 2 => {
                let wanted = intrinsic == Intrinsic::TEST_ASSERT_EQUAL;
                if (compare(arguments[0], arguments[1]) == 0) == wanted {
                    return success(void_value());
                }
                let mut message = HeapText::new();
                message.push_bytes(if wanted {
                    b"Test.assert_equal: the values differ"
                } else {
                    b"Test.assert_not_equal: the values are equal"
                });
                message.push_bytes(b"\nactual:\n");
                message.push_bytes(bytes_of(render(arguments[0], true)));
                message.push_bytes(b"\nexpected:\n");
                message.push_bytes(bytes_of(render(arguments[1], true)));
                message.push_bytes(b"\0");
                defect_with(message.data().cast(), node)
            }
            Intrinsic::TEST_ASSERT if arguments.len() == 2 && kind(0) == ValueKind::BOOLEAN && kind(1) == ValueKind::TEXT => {
                if (*arguments[0]).payload.boolean { success(void_value()) } else { message_defect(arguments[1], node) }
            }
            Intrinsic::TEST_FAIL if arguments.len() == 1 && kind(0) == ValueKind::TEXT => message_defect(arguments[0], node),
            _ => shape_defect(call_node),
        }
    }
}
