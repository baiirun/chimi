//! The outcome record (CHI-305 step 5, phase 8): the one JSON line a run
//! writes to `--outcome-file`, and the JSON form of the value it carries.
//! Both MUST match the interpreter's byte for byte; the language tests
//! compare the two backends' records.

use alloc::vec::Vec;
use core::ffi::c_char;

use crate::decimal::Decimal;
use crate::heap::layout::{Control, ControlKind, Field, MapNode, Node, Value, ValueKind};
use crate::heap::{list, map};
use crate::intrinsics::causes::{CANCELLATION_REASON, cause_has_detail, cause_text, control_cause};
use crate::intrinsics::debug::render;
use crate::intrinsics::random_host::chimi_runtime_random_read_v1;
use crate::telemetry::Out;

unsafe extern "C" {
    static CHIMI_NODES: [Node; 0];
    static CHIMI_TEST_COUNT: u32;
    fn chimi_runtime_exhausted_memory() -> !;
}

/// What the writer does next. It keeps its own stack, so a value of any
/// nesting depth costs heap, not host stack; the step pushed last runs first.
enum Step {
    Value(*const Value),
    Text(&'static [u8]),
    /// A field's `"name":`.
    Name(*const c_char),
    /// A Text map key's `"key":`.
    Key(*const Value),
}

struct Steps(Vec<Step>);

impl Steps {
    fn push(&mut self, step: Step) {
        if self.0.try_reserve(1).is_err() {
            // SAFETY: exits the process.
            unsafe { chimi_runtime_exhausted_memory() }
        }
        self.0.push(step);
    }

    /// `"name":value` pairs (or bare values), comma-separated, then `close`.
    ///
    /// # Safety
    /// `fields` holds `length` live fields.
    unsafe fn fields(&mut self, fields: *const Field, length: u32, named: bool, close: &'static [u8]) {
        self.push(Step::Text(close));
        for index in (0..length as usize).rev() {
            // SAFETY: the caller's contract.
            let field = unsafe { &*fields.add(index) };
            self.push(Step::Value(field.value));
            if named {
                self.push(Step::Name(field.name));
            }
            if index > 0 {
                self.push(Step::Text(b","));
            }
        }
    }

    /// A map, as the interpreter writes it: a Text-keyed or empty map is an
    /// object of its entries in key order, and any other map is
    /// `{"map":[[key,value],...]}` in key order.
    ///
    /// # Safety
    /// `root` is null or a live tree.
    unsafe fn map(&mut self, out: &Out, root: *const MapNode) {
        // SAFETY: the caller's contract.
        unsafe {
            let entries = map::entries(root);
            let text_keyed = entries.first().is_none_or(|node| (*(**node).key).kind == ValueKind::TEXT);
            out.bytes(if text_keyed { b"{" } else { b"{\"map\":[" });
            self.push(Step::Text(if text_keyed { b"}" } else { b"]}" }));
            for (index, &node) in entries.iter().enumerate().rev() {
                if text_keyed {
                    self.push(Step::Value((*node).value));
                    self.push(Step::Key((*node).key));
                    if index > 0 {
                        self.push(Step::Text(b","));
                    }
                } else {
                    self.push(Step::Text(b"]"));
                    self.push(Step::Value((*node).value));
                    self.push(Step::Text(b","));
                    self.push(Step::Value((*node).key));
                    self.push(Step::Text(if index > 0 { b",[" } else { b"[" }));
                }
            }
        }
    }
}

fn text_bytes(value: &Value) -> &[u8] {
    // SAFETY: a Text or Bytes value's buffer holds its length.
    unsafe {
        let text = value.payload.text;
        if text.length == 0 { &[] } else { core::slice::from_raw_parts(text.bytes.cast::<u8>(), text.length) }
    }
}

/// An opaque value: only what it is, as in the interpreter's public form.
fn opaque(out: &Out, name: &[u8]) {
    out.bytes(b"{\"type\":\"");
    out.bytes(name);
    out.bytes(b"\",\"fields\":{}}");
}

/// The JSON form of `root`.
///
/// # Safety
/// `root` is a live value.
unsafe fn json_value(out: &Out, root: *const Value) {
    let mut steps = Steps(Vec::new());
    steps.push(Step::Value(root));
    // SAFETY: the caller's contract; every value reached is live.
    unsafe {
        while let Some(step) = steps.0.pop() {
            let value = match step {
                Step::Text(text) => {
                    out.bytes(text);
                    continue;
                }
                Step::Name(name) => {
                    out.json_c(name);
                    out.bytes(b":");
                    continue;
                }
                Step::Key(key) => {
                    out.json(text_bytes(&*key));
                    out.bytes(b":");
                    continue;
                }
                Step::Value(value) => &*value,
            };
            let payload = &value.payload;
            match value.kind {
                ValueKind::VOID => out.bytes(b"{\"type\":\"Void\"}"),
                ValueKind::INTEGER => out.signed(payload.integer),
                ValueKind::FLOAT => out.bytes(crate::float::text(payload.floating).as_bytes()),
                ValueKind::TEXT => out.json(text_bytes(value)),
                ValueKind::BOOLEAN => out.bytes(if payload.boolean { b"true" } else { b"false" }),
                ValueKind::LIST => {
                    out.bytes(b"[");
                    steps.push(Step::Text(b"]"));
                    for index in (0..payload.list.length).rev() {
                        steps.push(Step::Value(list::get(value, index)));
                        if index > 0 {
                            steps.push(Step::Text(b","));
                        }
                    }
                }
                ValueKind::RECORD => {
                    out.bytes(b"{");
                    steps.fields(payload.record.fields, payload.record.length, true, b"}");
                }
                ValueKind::DURATION | ValueKind::INSTANT | ValueKind::TIMESTAMP => {
                    out.bytes(match value.kind {
                        ValueKind::DURATION => b"{\"duration\":",
                        ValueKind::INSTANT => b"{\"instant\":",
                        _ => b"{\"timestamp\":",
                    });
                    out.signed(payload.integer);
                    out.bytes(b"}");
                }
                ValueKind::DECIMAL => {
                    let raw = payload.decimal;
                    let magnitude = (u128::from(raw.high) << 64) | u128::from(raw.low);
                    let Some(decimal) = Decimal::new(raw.negative, magnitude, raw.scale) else { libc::abort() };
                    out.bytes(b"{\"decimal\":");
                    out.json(decimal.normalized().text().as_bytes());
                    out.bytes(b"}");
                }
                ValueKind::RANGE => {
                    out.bytes(b"{\"range\":{\"start\":");
                    out.signed(payload.range.start);
                    out.bytes(b",\"end\":");
                    out.signed(payload.range.end);
                    out.bytes(b"}}");
                }
                ValueKind::TUPLE => {
                    out.bytes(b"{\"tuple\":[");
                    steps.fields(payload.record.fields, payload.record.length, false, b"]}");
                }
                ValueKind::CONSTRUCTED => {
                    let constructed = payload.constructed;
                    out.bytes(b"{\"type\":");
                    out.json_c(constructed.type_name);
                    if !constructed.variant_name.is_null() {
                        out.bytes(b",\"variant\":");
                        out.json_c(constructed.variant_name);
                    }
                    out.bytes(b",\"fields\":{");
                    steps.fields(constructed.fields, constructed.length, true, b"}}");
                }
                ValueKind::CALLABLE => {
                    out.bytes(b"{\"callable\":");
                    out.json_c((*payload.callable).name);
                    out.bytes(b"}");
                }
                ValueKind::MAP => steps.map(out, payload.map),
                // A set is a list of its items in ascending order.
                ValueKind::SET => {
                    let entries = map::entries(payload.map);
                    out.bytes(b"[");
                    steps.push(Step::Text(b"]"));
                    for (index, &node) in entries.iter().enumerate().rev() {
                        steps.push(Step::Value((*node).key));
                        if index > 0 {
                            steps.push(Step::Text(b","));
                        }
                    }
                }
                ValueKind::JSON_NUMBER => out.json_c(payload.json_number),
                // Lowercase hex, two digits per byte.
                ValueKind::BYTES => {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    out.bytes(b"{\"bytes\":\"");
                    for &byte in text_bytes(value) {
                        out.bytes(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]]);
                    }
                    out.bytes(b"\"}");
                }
                ValueKind::TASK => {
                    out.bytes(b"{\"task\":");
                    out.unsigned(payload.task);
                    out.bytes(b"}");
                }
                ValueKind::CHANNEL_SENDER | ValueKind::CHANNEL_RECEIVER | ValueKind::CHANNEL_COMPLETION => {
                    out.bytes(b"{\"channel\":");
                    out.unsigned(payload.capability.channel);
                    out.bytes(b"}");
                }
                ValueKind::PROCESS_ADDRESS => opaque(out, b"Address"),
                ValueKind::PROCESS_INBOX => opaque(out, b"Inbox"),
                ValueKind::SUPERVISOR_RUNNING => opaque(out, b"Running"),
                ValueKind::MANAGED_PROCESS => opaque(out, b"ManagedProcess"),
                ValueKind::MANAGED_SUPERVISOR => opaque(out, b"ManagedSupervisor"),
                ValueKind::PROCESS_MONITOR => opaque(out, b"Monitor"),
                ValueKind::TEST_RECORDER => opaque(out, b"Recorder"),
                ValueKind::TEST_SCRIPT => opaque(out, b"Script"),
                ValueKind::TEST_MAILBOX => opaque(out, b"Mailbox"),
                ValueKind::CAUSE => opaque(out, b"Cause"),
                ValueKind::NATIVE_HANDLE => {
                    out.bytes(b"{\"type\":");
                    out.json_c(payload.handle.name);
                    out.bytes(b",\"fields\":{}}");
                }
                _ => {}
            }
        }
    }
}

/// Writes the outcome record of `control` to `output`.
///
/// # Safety
/// `output` is an open stream and `control` a live outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_print_outcome_v1(output: *mut libc::FILE, control: *const Control) {
    let out = Out(output);
    // SAFETY: the caller's contract.
    unsafe {
        let control = &*control;
        out.bytes(b"{\"schema\":\"chimi.runtime.v1\",");
        match control.kind {
            ControlKind::SUCCESS => {
                out.bytes(b"\"outcome\":\"success\",\"value\":");
                json_value(&out, control.value);
            }
            ControlKind::FAILURE => {
                out.bytes(b"\"outcome\":\"expected_failure\",\"error\":");
                json_value(&out, control.value);
                // A test build also writes the debug rendering `chimi test` reports.
                if CHIMI_TEST_COUNT > 0 {
                    let rendered = render(control.value, true);
                    out.bytes(b",\"rendering\":");
                    out.json_c((*rendered).payload.text.bytes);
                }
            }
            ControlKind::DEFECT => {
                out.bytes(b"\"outcome\":\"defect\",\"message\":");
                out.json_c(control.message);
                if control.node < 0 {
                    out.bytes(b",\"core_node_id\":null,\"origin_id\":null");
                } else {
                    let node = &*(&raw const CHIMI_NODES).cast::<Node>().add(control.node as usize);
                    out.bytes(b",\"core_node_id\":");
                    out.signed(control.node);
                    out.bytes(b",\"origin_id\":");
                    out.unsigned(u64::from(node.origin));
                }
            }
            ControlKind::CANCEL => {
                out.bytes(b"\"outcome\":\"cancellation\",\"reason\":");
                out.json(CANCELLATION_REASON.to_bytes());
            }
            _ => {}
        }
        // `chimi test` names the seed in the report of a failing test that drew one.
        if chimi_runtime_random_read_v1() {
            out.bytes(b",\"random_read\":true");
        }
        if control.kind != ControlKind::SUCCESS {
            let cause = control_cause(control);
            if !cause.is_null() && cause_has_detail(cause) {
                out.bytes(b",\"cause\":");
                out.json(text_bytes(&*cause_text(cause)));
            }
        }
        out.bytes(b"}\n");
    }
}
