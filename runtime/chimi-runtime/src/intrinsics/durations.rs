//! The native core of std/duration, over [`crate::duration`]'s text form
//! and parser; `execution/duration_operations.rs` is the interpreter's, and
//! the overflow messages match it.

use super::message::Message;
use super::standard::{defect_with, failure, shape_defect, standard_value, success};
use crate::duration::{self, ParseFailure, Unit};
use crate::heap::layout::{Control, Intrinsic, StandardConstructor, Value, ValueKind};
use crate::heap::values::{integer_value, text_value};

/// A Duration.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn duration_value(nanoseconds: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::DURATION, nanoseconds) }
}

/// The unit of a constructor or `to_` intrinsic `offset` places after the
/// first of its run, which is nanoseconds.
fn unit_from_nanoseconds(offset: u32) -> Unit {
    Unit::DESCENDING[5 - offset as usize]
}

/// The defect "`Duration.<function>` overflows the Duration range: <operation>",
/// where `operation` is written by `write`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn overflow(function: &str, operation: impl FnOnce(Message) -> Message, node: i64) -> *mut Control {
    let message = Message::new().text(b"`Duration.").text(function.as_bytes()).text(b"` overflows the Duration range: ");
    // SAFETY: the caller's contract.
    unsafe { defect_with(operation(message).finish(), node) }
}

/// std/duration's intrinsic `intrinsic` on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn duration_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let kind_of = |index: usize| arguments.get(index).map(|&value| (*value).kind);
        if (Intrinsic::DURATION_NANOSECONDS..=Intrinsic::DURATION_HOURS).contains(&intrinsic) {
            if arguments.len() != 1 || kind_of(0) != Some(ValueKind::INTEGER) {
                return shape_defect(call_node);
            }
            let unit = unit_from_nanoseconds(intrinsic - Intrinsic::DURATION_NANOSECONDS);
            let count = (*arguments[0]).payload.integer;
            return match count.checked_mul(unit.nanoseconds()) {
                Some(result) => success(duration_value(result)),
                None => {
                    let message = Message::new()
                        .text(b"`Duration.")
                        .text(unit.constructor().as_bytes())
                        .text(b"(")
                        .integer(count)
                        .text(b")` overflows the Duration range");
                    defect_with(message.finish(), node)
                }
            };
        }
        if intrinsic == Intrinsic::DURATION_PARSE {
            if arguments.len() != 1 || kind_of(0) != Some(ValueKind::TEXT) {
                return shape_defect(call_node);
            }
            let text = (*arguments[0]).payload.text;
            let bytes = if text.length == 0 {
                &[][..]
            } else {
                core::slice::from_raw_parts(text.bytes.cast::<u8>(), text.length)
            };
            // Bytes that are not UTF-8 are Invalid.
            let parsed = core::str::from_utf8(bytes).map_err(|_| ParseFailure::Invalid).and_then(duration::parse);
            return match parsed {
                Ok(nanoseconds) => success(duration_value(nanoseconds)),
                Err(reason) => failure(standard_value(
                    match reason {
                        ParseFailure::Empty => StandardConstructor::DURATION_PARSE_EMPTY,
                        ParseFailure::Invalid => StandardConstructor::DURATION_PARSE_INVALID,
                        ParseFailure::OutOfRange => StandardConstructor::DURATION_PARSE_OUT_OF_RANGE,
                    },
                    &[],
                )),
            };
        }
        if kind_of(0) != Some(ValueKind::DURATION) {
            return shape_defect(call_node);
        }
        let value = (*arguments[0]).payload.integer;
        let left = duration::text(value);
        let left = left.as_bytes();
        let single = arguments.len() == 1;
        if (Intrinsic::DURATION_TO_NANOSECONDS..=Intrinsic::DURATION_TO_SECONDS).contains(&intrinsic) {
            if !single {
                return shape_defect(call_node);
            }
            let unit = unit_from_nanoseconds(intrinsic - Intrinsic::DURATION_TO_NANOSECONDS);
            return success(integer_value(ValueKind::INTEGER, duration::whole_units(value, unit)));
        }
        let pair_of = |kind: ValueKind| arguments.len() == 2 && kind_of(1) == Some(kind);
        match intrinsic {
            Intrinsic::DURATION_ADD | Intrinsic::DURATION_SUBTRACT if pair_of(ValueKind::DURATION) => {
                let add = intrinsic == Intrinsic::DURATION_ADD;
                let other = (*arguments[1]).payload.integer;
                let result = if add { value.checked_add(other) } else { value.checked_sub(other) };
                match result {
                    Some(result) => success(duration_value(result)),
                    None => {
                        let right = duration::text(other);
                        let sign: &[u8] = if add { b" + " } else { b" - " };
                        overflow(
                            if add { "add" } else { "subtract" },
                            |message| message.text(left).text(sign).text(right.as_bytes()),
                            node,
                        )
                    }
                }
            }
            Intrinsic::DURATION_MULTIPLY if pair_of(ValueKind::INTEGER) => {
                let other = (*arguments[1]).payload.integer;
                match value.checked_mul(other) {
                    Some(result) => success(duration_value(result)),
                    None => overflow("multiply", |message| message.text(left).text(b" * ").integer(other), node),
                }
            }
            Intrinsic::DURATION_DIVIDE if pair_of(ValueKind::INTEGER) => {
                let other = (*arguments[1]).payload.integer;
                if other == 0 {
                    let message = Message::new().text(b"`Duration.divide` by zero: ").text(left).text(b" / 0");
                    return defect_with(message.finish(), node);
                }
                match value.checked_div(other) {
                    Some(result) => success(duration_value(result)),
                    None => overflow("divide", |message| message.text(left).text(b" / ").integer(other), node),
                }
            }
            Intrinsic::DURATION_NEGATE | Intrinsic::DURATION_ABSOLUTE if single => {
                let negate = intrinsic == Intrinsic::DURATION_NEGATE;
                if value == i64::MIN {
                    return if negate {
                        overflow("negate", |message| message.text(b"-(").text(left).text(b")"), node)
                    } else {
                        overflow("absolute", |message| message.text(b"|").text(left).text(b"|"), node)
                    };
                }
                success(duration_value(if negate || value < 0 { -value } else { value }))
            }
            Intrinsic::DURATION_TO_TEXT if single => success(text_value(left.as_ptr().cast(), left.len())),
            _ => shape_defect(call_node),
        }
    }
}
