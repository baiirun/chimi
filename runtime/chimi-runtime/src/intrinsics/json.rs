//! The native core of std/json: `Json.decode` builds values from the tape of
//! [`crate::json::decode`], `Json.encode` writes a `Json.Value` as text, and
//! the `Json.Number` conversions read a number's exact value from its
//! spelling. `execution/json.rs` is the interpreter's. Every value is
//! allocated in the order `runtime.c` allocated it, so memory reports do not
//! change.

use core::ffi::{CStr, c_char};
use core::ptr;

use alloc::vec;
use alloc::vec::Vec;

use super::standard::{boolean_value, failure, shape_defect, standard_constructor_of, standard_value, success};
use super::message::integer_spelling;
use super::text::HeapText;
use crate::decimal::{self, Decimal};
use crate::heap::allocator::allocate;
use crate::heap::layout::{Control, Intrinsic, MapNode, StandardConstructor, Value, ValueKind};
use crate::heap::values::{decimal_value, integer_value, json_number_value, pointer_value, text_value};
use crate::heap::{list, map};
use crate::json::NodeKind;

unsafe extern "C" {
    /// Prints "Chimi runtime: malformed JSON tape" and exits 70.
    fn chimi_runtime_json_tape_defect_v1() -> !;
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// A written exponent past this is clamped to it, as in the interpreter: only
/// numbers far outside Int and Decimal reach it.
const EXPONENT_CLAMP: i64 = 1_000_000_000_000_000;

/// The bytes of a Text value.
///
/// # Safety
/// `text` is a live Text.
unsafe fn text_bytes<'a>(text: *const Value) -> &'a [u8] {
    // SAFETY: the caller's contract; a Text's bytes are never null.
    unsafe {
        let payload = (*text).payload.text;
        core::slice::from_raw_parts(payload.bytes.cast::<u8>(), payload.length)
    }
}

/// The spelling of a JSON number value, without its NUL.
///
/// # Safety
/// `number` is a live JSON number.
unsafe fn number_spelling<'a>(number: *const Value) -> &'a [u8] {
    // SAFETY: the caller's contract; the spelling is NUL-terminated.
    unsafe { CStr::from_ptr((*number).payload.json_number).to_bytes() }
}

/// `Json.decode`: the value the text holds, or `DecodeError.InvalidJson`.
///
/// # Safety
/// Runs on a fiber that may allocate; `text` is a live Text.
unsafe fn decode(text: *const Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let source = text_bytes(text);
        match crate::json::decode(source) {
            Ok(tape) => success(build(&tape, source)),
            Err(error) => {
                let message = text_value(error.message.as_ptr().cast::<c_char>(), error.message.len());
                let offset = integer_value(ValueKind::INTEGER, error.offset as i64);
                failure(standard_value(StandardConstructor::DECODE_INVALID_JSON, &[message, offset]))
            }
        }
    }
}

/// The value `tape` describes: each node's members are the values on top of
/// the stack when it is reached. The tape's shape is the decoder's contract;
/// a node that breaks it ends the program rather than reading past the stack.
///
/// # Safety
/// Runs on a fiber that may allocate; `tape` was decoded from `source`.
unsafe fn build(tape: &crate::json::Tape, source: &[u8]) -> *mut Value {
    let mut stack: Vec<*mut Value> = Vec::new();
    if stack.try_reserve_exact(tape.nodes.len()).is_err() {
        // SAFETY: exits.
        unsafe { chimi_runtime_exhausted_memory() }
    }
    // SAFETY: the caller's contract; every slice stays within its bytes, as
    // the decoder located each node.
    unsafe {
        for node in &tape.nodes {
            let bytes = match node.kind {
                NodeKind::DecodedText | NodeKind::DecodedKey => &tape.decoded[..],
                _ => source,
            };
            let located = || bytes.get(node.offset..node.offset + node.length).unwrap_or_else(|| chimi_runtime_json_tape_defect_v1());
            let value = match node.kind {
                NodeKind::Null => standard_value(StandardConstructor::JSON_NULL, &[]),
                NodeKind::False | NodeKind::True => {
                    standard_value(StandardConstructor::JSON_BOOLEAN, &[boolean_value(node.kind == NodeKind::True)])
                }
                NodeKind::Number => {
                    let spelling = located();
                    let number = allocate(spelling.len() + 1, 1);
                    ptr::copy_nonoverlapping(spelling.as_ptr(), number, spelling.len());
                    *number.add(spelling.len()) = 0;
                    standard_value(StandardConstructor::JSON_NUMBER, &[json_number_value(number.cast())])
                }
                NodeKind::SourceText | NodeKind::DecodedText => {
                    let text = located();
                    standard_value(StandardConstructor::JSON_TEXT, &[text_value(text.as_ptr().cast(), text.len())])
                }
                NodeKind::SourceKey | NodeKind::DecodedKey => {
                    let text = located();
                    text_value(text.as_ptr().cast(), text.len())
                }
                NodeKind::Array => {
                    let Some(start) = stack.len().checked_sub(node.length).filter(|_| node.length <= u32::MAX as usize) else {
                        chimi_runtime_json_tape_defect_v1()
                    };
                    let length = node.length as u32;
                    let items = list::list_value(list::from_values(stack.as_ptr().add(start), length), 0, length);
                    stack.truncate(start);
                    standard_value(StandardConstructor::JSON_ARRAY, &[items])
                }
                NodeKind::Object => {
                    let Some(start) = node.length.checked_mul(2).and_then(|members| stack.len().checked_sub(members)) else {
                        chimi_runtime_json_tape_defect_v1()
                    };
                    let mut tree: *mut MapNode = ptr::null_mut();
                    for member in stack[start..].chunks_exact(2) {
                        tree = map::put(tree, member[0], member[1]);
                    }
                    stack.truncate(start);
                    standard_value(StandardConstructor::JSON_OBJECT, &[pointer_value(ValueKind::MAP, tree.cast())])
                }
            };
            stack.push(value);
        }
        match stack[..] {
            [root] => root,
            _ => chimi_runtime_json_tape_defect_v1(),
        }
    }
}

/// One pending piece of `Json.encode`'s output; the last pushed runs first.
enum Step {
    /// A literal.
    Text(&'static [u8]),
    /// An Object entry's member name and `:`.
    Key(*const MapNode),
    Value(*const Value),
}

/// Appends `text` as a JSON string.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn push_json_string(output: &mut HeapText, text: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    // SAFETY: the caller's contract.
    unsafe {
        output.push_bytes(b"\"");
        for &byte in text {
            match byte {
                b'"' => output.push_bytes(b"\\\""),
                b'\\' => output.push_bytes(b"\\\\"),
                0x08 => output.push_bytes(b"\\b"),
                0x0c => output.push_bytes(b"\\f"),
                b'\n' => output.push_bytes(b"\\n"),
                b'\r' => output.push_bytes(b"\\r"),
                b'\t' => output.push_bytes(b"\\t"),
                0..0x20 => output.push_bytes(&[b'\\', b'u', b'0', b'0', HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]]),
                _ => output.push_bytes(&[byte]),
            }
        }
        output.push_bytes(b"\"");
    }
}

/// Writes one `Json.Value`, queueing its members; false when it is not a
/// well-formed one.
///
/// # Safety
/// Runs on a fiber that may allocate; `value` is live.
unsafe fn encode_step(
    output: &mut HeapText,
    steps: &mut Vec<Step>,
    entries: &mut Vec<Vec<*const MapNode>>,
    value: *const Value,
) -> bool {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        if (*value).kind != ValueKind::CONSTRUCTED {
            return false;
        }
        let constructed = (*value).payload.constructed;
        let field = if constructed.length == 0 { None } else { Some((*constructed.fields).value) };
        let field_of = |kind: ValueKind| field.filter(|&field| (*field).kind == kind);
        match standard_constructor_of(constructed.constructor) {
            StandardConstructor::JSON_NULL => output.push_bytes(b"null"),
            StandardConstructor::JSON_BOOLEAN => match field_of(ValueKind::BOOLEAN) {
                Some(field) => output.push_bytes(if (*field).payload.boolean { b"true" } else { b"false" }),
                None => return false,
            },
            StandardConstructor::JSON_NUMBER => match field_of(ValueKind::JSON_NUMBER) {
                Some(field) => output.push_bytes(number_spelling(field)),
                None => return false,
            },
            StandardConstructor::JSON_TEXT => match field_of(ValueKind::TEXT) {
                Some(field) => push_json_string(output, text_bytes(field)),
                None => return false,
            },
            StandardConstructor::JSON_ARRAY => {
                let Some(items) = field_of(ValueKind::LIST) else { return false };
                output.push_bytes(b"[");
                steps.push(Step::Text(b"]"));
                for index in (0..(*items).payload.list.length).rev() {
                    steps.push(Step::Value(list::get(items, index)));
                    if index != 0 {
                        steps.push(Step::Text(b","));
                    }
                }
            }
            StandardConstructor::JSON_OBJECT => {
                let Some(members) = field_of(ValueKind::MAP) else { return false };
                output.push_bytes(b"{");
                let nodes = map::entries((*members).payload.map);
                steps.push(Step::Text(b"}"));
                for (index, &node) in nodes.iter().enumerate().rev() {
                    steps.push(Step::Value((*node).value));
                    steps.push(Step::Key(node));
                    if index != 0 {
                        steps.push(Step::Text(b","));
                    }
                }
                // Kept until the encoding ends, as the C freed its entry
                // arrays only then.
                entries.push(nodes);
            }
            _ => return false,
        }
        true
    }
}

/// `Json.encode`, with its own stack of steps so a program-built value of
/// any nesting depth costs heap, not stack. False when `root` is not a
/// well-formed `Json.Value` or an Object key is not Text.
///
/// # Safety
/// Runs on a fiber that may allocate; `root` is live.
unsafe fn encode(output: &mut HeapText, root: *const Value) -> bool {
    let mut steps = vec![Step::Value(root)];
    let mut entries = Vec::new();
    // SAFETY: the caller's contract; queued nodes and values are live.
    unsafe {
        while let Some(step) = steps.pop() {
            match step {
                Step::Text(text) => output.push_bytes(text),
                Step::Key(node) => {
                    let key = (*node).key;
                    if (*key).kind != ValueKind::TEXT {
                        return false;
                    }
                    push_json_string(output, text_bytes(key));
                    output.push_bytes(b":");
                }
                Step::Value(value) => {
                    if !encode_step(output, &mut steps, &mut entries, value) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// The exact value of a JSON number as ±`significant` × 10^`exponent`,
/// where `significant` (the spelling's digits, `.` skipped) has no leading
/// or trailing zeros and is empty for zero, and the scale the spelling
/// writes: fraction digits minus the written exponent.
struct ExactNumber<'a> {
    negative: bool,
    significant: &'a [u8],
    exponent: i64,
    written_scale: i64,
}

/// Reads `spelling`, a number the decoder or `Json.int` wrote. The digits
/// are gathered in a heap block, as the C did, so memory reports do not
/// change.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn exact_number<'a>(spelling: &[u8]) -> ExactNumber<'a> {
    let (negative, text) = match spelling.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, spelling),
    };
    let mantissa = text.iter().position(|&byte| byte == b'e' || byte == b'E').unwrap_or(text.len());
    let point = text[..mantissa].iter().position(|&byte| byte == b'.').unwrap_or(mantissa);
    let fraction_digits = if point < mantissa { (mantissa - point - 1) as i64 } else { 0 };
    let mut written = 0i64;
    if let Some((_, exponent)) = text[mantissa..].split_first() {
        let (written_negative, digits) = match exponent.split_first() {
            Some((b'-', rest)) => (true, rest),
            Some((b'+', rest)) => (false, rest),
            _ => (false, exponent),
        };
        for &digit in digits {
            written = (written * 10 + i64::from(digit - b'0')).min(EXPONENT_CLAMP);
        }
        if written_negative {
            written = -written;
        }
    }
    // SAFETY: the caller's contract; the block holds every mantissa byte.
    let significant = unsafe {
        let block = allocate(mantissa + 1, 1);
        let mut length = 0;
        for &byte in &text[..mantissa] {
            if byte != b'.' && (length > 0 || byte != b'0') {
                *block.add(length) = byte;
                length += 1;
            }
        }
        core::slice::from_raw_parts(block, length)
    };
    let trimmed = significant.iter().rposition(|&byte| byte != b'0').map_or(0, |last| last + 1);
    let trailing = (significant.len() - trimmed) as i64;
    ExactNumber {
        negative,
        significant: &significant[..trimmed],
        exponent: written - fraction_digits + trailing,
        written_scale: fraction_digits - written,
    }
}

/// Why a JSON number is no Int.
enum NotInt {
    NotInteger,
    OutOfRange,
}

/// `Json.Number.to_int`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn number_to_int(spelling: &[u8]) -> Result<i64, NotInt> {
    // SAFETY: the caller's contract.
    let number = unsafe { exact_number(spelling) };
    let length = number.significant.len() as i64;
    if length == 0 {
        return Ok(0);
    }
    if number.exponent < 0 {
        return Err(NotInt::NotInteger);
    }
    if length + number.exponent > 19 {
        return Err(NotInt::OutOfRange);
    }
    let mut magnitude = 0u128;
    for &digit in number.significant {
        magnitude = magnitude * 10 + u128::from(digit - b'0');
    }
    for _ in 0..number.exponent {
        magnitude *= 10;
    }
    if magnitude > i64::MAX as u128 + u128::from(number.negative) {
        return Err(NotInt::OutOfRange);
    }
    Ok(if number.negative { 0u64.wrapping_sub(magnitude as u64) as i64 } else { magnitude as i64 })
}

/// `Json.Number.to_decimal`: the exact value at the spelling's own scale,
/// or the largest smaller scale that fits; `None` past 38 significant or
/// fraction digits.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn number_to_decimal(spelling: &[u8]) -> Option<Decimal> {
    // SAFETY: the caller's contract.
    let number = unsafe { exact_number(spelling) };
    let max_scale = i64::from(decimal::MAX_SCALE);
    let length = number.significant.len() as i64;
    if length == 0 {
        return Decimal::new(false, 0, number.written_scale.clamp(0, max_scale) as u32);
    }
    if length + number.exponent.max(0) > max_scale || -number.exponent > max_scale {
        return None;
    }
    // At least the smallest scale, max(0, -exponent).
    let scale = number.written_scale.clamp(0, max_scale).min(max_scale - length - number.exponent);
    let mut magnitude = 0u128;
    for &digit in number.significant {
        magnitude = magnitude * 10 + u128::from(digit - b'0');
    }
    for _ in 0..number.exponent + scale {
        magnitude *= 10;
    }
    Decimal::new(number.negative, magnitude, scale as u32)
}

/// std/json's intrinsic `intrinsic` on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn json_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let kind = |kind: ValueKind| matches!(arguments, [only] if (**only).kind == kind);
        match intrinsic {
            Intrinsic::JSON_DECODE if kind(ValueKind::TEXT) => decode(arguments[0]),
            Intrinsic::JSON_ENCODE if arguments.len() == 1 => {
                let mut output = HeapText::new();
                if !encode(&mut output, arguments[0]) {
                    return shape_defect(call_node);
                }
                success(text_value(output.data().cast(), output.len()))
            }
            Intrinsic::JSON_INT if kind(ValueKind::INTEGER) => {
                // A 32-byte block, as the C's snprintf buffer was.
                let number = allocate(32, 1);
                let mut spelling = [0; 20];
                let digits = integer_spelling((*arguments[0]).payload.integer, &mut spelling);
                ptr::copy_nonoverlapping(digits.as_ptr(), number, digits.len());
                *number.add(digits.len()) = 0;
                success(standard_value(StandardConstructor::JSON_NUMBER, &[json_number_value(number.cast())]))
            }
            Intrinsic::JSON_NUMBER_TO_INT if kind(ValueKind::JSON_NUMBER) => {
                match number_to_int(number_spelling(arguments[0])) {
                    Ok(value) => success(integer_value(ValueKind::INTEGER, value)),
                    Err(reason) => failure(standard_value(
                        match reason {
                            NotInt::NotInteger => StandardConstructor::JSON_NOT_INTEGER,
                            NotInt::OutOfRange => StandardConstructor::JSON_OUT_OF_RANGE,
                        },
                        &[],
                    )),
                }
            }
            Intrinsic::JSON_NUMBER_TO_DECIMAL if kind(ValueKind::JSON_NUMBER) => {
                match number_to_decimal(number_spelling(arguments[0])) {
                    Some(value) => success(decimal_value(super::decimals::payload(value))),
                    None => failure(standard_value(StandardConstructor::JSON_OUT_OF_RANGE, &[])),
                }
            }
            _ => shape_defect(call_node),
        }
    }
}
