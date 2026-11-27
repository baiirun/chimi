//! The debug format of std/debug (G10). It MUST match the interpreter's
//! renderer in `execution/debug.rs` byte for byte; the language tests compare
//! both. Captures and capability contents are never shown.
//!
//! The renderer keeps its own stack of steps, so a value of any nesting depth
//! costs heap, not stack: `Debug.render` may run on a 64 KiB fiber stack. A
//! step pushed last runs first. The text grows as `runtime.c`'s `TextBuffer`
//! did, and the one other heap allocation, a record's field order, comes at
//! the same point, so memory reports do not change.

use alloc::vec::Vec;
use core::ffi::{CStr, c_char};

use super::message::integer_spelling;
use super::standard::{standard_constructor_of, symbol_name};
use super::text::HeapText;
use crate::heap::allocator::allocate;
use crate::heap::layout::{CallableKind, Field, StandardConstructor, Value, ValueKind};
use crate::heap::values::text_value;
use crate::heap::{json_number, list, map};
use crate::{decimal, duration, float, timestamp};

unsafe extern "C" {
    /// The `<module path>.<identifier>` label of a scheduler name id, or
    /// null for an id that names no process name.
    fn chimi_runtime_process_name_label_v1(name: u64) -> *const c_char;
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// `CHIMI_ADDRESS_NAMED` (chimi_fiber.h).
const ADDRESS_NAMED: u32 = 1;
/// `CHIMI_ADDRESS_TEST` (runtime.c): a std/test Mailbox's address.
const ADDRESS_TEST: u32 = 3;

#[derive(Clone, Copy)]
enum Step {
    Value(*const Value),
    /// A field name, written as `name: `.
    Label(*const c_char),
    /// A map key, which renders on one line even in the indented format, as
    /// the interpreter does.
    Key(*const Value),
    /// Restores the format a key interrupted, then writes `: `.
    KeyEnd(bool),
    Separator { index: u32, spaced: bool },
    PartEnd,
    Close { close: &'static [u8], spaced: bool },
}

struct Renderer {
    output: HeapText,
    steps: Vec<Step>,
    indented: bool,
    depth: u32,
}

/// The bytes of a C string.
///
/// # Safety
/// `text` is a NUL-terminated string that outlives the render.
unsafe fn c_bytes<'a>(text: *const c_char) -> &'a [u8] {
    // SAFETY: the caller's contract.
    unsafe { CStr::from_ptr(text).to_bytes() }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

impl Renderer {
    fn push(&mut self, step: Step) {
        if self.steps.len() == self.steps.capacity() && self.steps.try_reserve(self.steps.len().max(64)).is_err() {
            // SAFETY: exits.
            unsafe { chimi_runtime_exhausted_memory() }
        }
        self.steps.push(step);
    }

    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn text(&mut self, bytes: &[u8]) {
        // SAFETY: the caller's contract.
        unsafe { self.output.push_bytes(bytes) }
    }

    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn line(&mut self) {
        // SAFETY: the caller's contract.
        unsafe {
            self.text(b"\n");
            for _ in 0..self.depth {
                self.text(b"  ");
            }
        }
    }

    /// Between parts: `, ` on one line, or a line break and indentation.
    /// Before the first part on one line, a space when `spaced`.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn separator(&mut self, index: u32, spaced: bool) {
        // SAFETY: the caller's contract.
        unsafe {
            if self.indented {
                self.line();
            } else if index > 0 {
                self.text(b", ");
            } else if spaced {
                self.text(b" ");
            }
        }
    }

    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn close(&mut self, close: &[u8], spaced: bool) {
        // SAFETY: the caller's contract.
        unsafe {
            if self.indented {
                self.line();
            } else if spaced {
                self.text(b" ");
            }
            self.text(close);
        }
    }

    /// Opens a composite of `length` parts: each part is a separator, an
    /// optional label, its value and a part end, and `close` follows the
    /// last. Returns false when there are no parts, after writing `close`.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn open(&mut self, open: &[u8], close: &'static [u8], spaced: bool, length: u32) -> bool {
        // SAFETY: the caller's contract.
        unsafe {
            self.text(open);
            if length == 0 {
                self.text(close);
                return false;
            }
        }
        self.depth += 1;
        self.push(Step::Close { close, spaced });
        true
    }

    /// Queues part `index`; parts are queued last to first.
    fn part(&mut self, index: u32, spaced: bool, label: Option<*const c_char>, value: *const Value) {
        self.push(Step::PartEnd);
        self.push(Step::Value(value));
        if let Some(label) = label {
            self.push(Step::Label(label));
        }
        self.push(Step::Separator { index, spaced });
    }

    /// Named parts in the given order, as `name: value`; `labels` false
    /// drops the names.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate; `fields` holds `length` fields and
    /// `order`, when given, `length` indexes into them.
    unsafe fn fields(
        &mut self,
        (open, close): (&[u8], &'static [u8]),
        spaced: bool,
        fields: *const Field,
        order: Option<*const u32>,
        length: u32,
        labels: bool,
    ) {
        // SAFETY: the caller's contract.
        unsafe {
            if !self.open(open, close, spaced, length) {
                return;
            }
            for index in (0..length).rev() {
                let at = order.map_or(index, |order| *order.add(index as usize));
                let field = &*fields.add(at as usize);
                self.part(index, spaced, labels.then_some(field.name), field.value);
            }
        }
    }

    /// A Text in quotes, with quotes, backslashes, control characters and a
    /// `${` escaped as a Chimi literal would spell them.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn quote(&mut self, text: &[u8]) {
        // SAFETY: the caller's contract.
        unsafe {
            self.text(b"\"");
            for (at, &byte) in text.iter().enumerate() {
                match byte {
                    b'"' => self.text(b"\\\""),
                    b'\\' => self.text(b"\\\\"),
                    b'\n' => self.text(b"\\n"),
                    b'\r' => self.text(b"\\r"),
                    b'\t' => self.text(b"\\t"),
                    b'$' if text.get(at + 1) == Some(&b'{') => self.text(b"\\$"),
                    0..0x20 | 0x7f => {
                        let digits = [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]];
                        self.text(b"\\u{");
                        self.text(if byte < 0x10 { &digits[1..] } else { &digits });
                        self.text(b"}");
                    }
                    _ => self.text(&[byte]),
                }
            }
            self.text(b"\"");
        }
    }

    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn integer(&mut self, value: i64) {
        let mut spelling = [0; 20];
        // SAFETY: the caller's contract.
        unsafe { self.text(integer_spelling(value, &mut spelling)) }
    }

    /// The map or set `root`'s entries, as `Map {k: v, ...}` or `Set {k, ...}`.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate; `root` is null or a live tree.
    unsafe fn entries(&mut self, root: *const crate::heap::layout::MapNode, with_values: bool) {
        // SAFETY: the caller's contract.
        unsafe {
            let entries = map::entries(root);
            let open: &[u8] = if with_values { b"Map {" } else { b"Set {" };
            if !self.open(open, b"}", true, entries.len() as u32) {
                return;
            }
            for (index, &node) in entries.iter().enumerate().rev() {
                self.push(Step::PartEnd);
                if with_values {
                    self.push(Step::Value((*node).value));
                    self.push(Step::Key((*node).key));
                } else {
                    self.push(Step::Value((*node).key));
                }
                self.push(Step::Separator { index: index as u32, spaced: true });
            }
        }
    }

    /// Writes a scalar, or queues the parts of a composite.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate; `value` is live.
    unsafe fn expand(&mut self, value: *const Value) {
        use ValueKind as K;
        // SAFETY: the caller's contract; payloads are read as their kinds say.
        unsafe {
            let payload = &(*value).payload;
            match (*value).kind {
                K::VOID => self.text(b"Void"),
                K::INTEGER => self.integer(payload.integer),
                K::FLOAT => {
                    let raw = payload.floating;
                    self.text(float::text(if raw == 0.0 { 0.0 } else { raw }).as_bytes());
                }
                K::TEXT => {
                    let text = payload.text;
                    self.quote(core::slice::from_raw_parts(text.bytes.cast(), text.length));
                }
                // Two hex digits per byte, as a hex dump shows them: `Bytes(68 69)`.
                K::BYTES => {
                    let bytes = payload.text;
                    self.text(b"Bytes(");
                    for index in 0..bytes.length {
                        let byte = *bytes.bytes.cast::<u8>().add(index);
                        let digits = [b' ', HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]];
                        self.text(if index > 0 { &digits } else { &digits[1..] });
                    }
                    self.text(b")");
                }
                K::BOOLEAN => self.text(if payload.boolean { b"True" } else { b"False" }),
                K::LIST => {
                    let length = payload.list.length;
                    if self.open(b"[", b"]", false, length) {
                        for index in (0..length).rev() {
                            self.part(index, false, None, list::get(value, index));
                        }
                    }
                }
                K::RECORD => {
                    let record = payload.record;
                    let length = record.length as usize;
                    let order = allocate(length, size_of::<u32>()).cast::<u32>();
                    let name = |index: u32| c_bytes((*record.fields.add(index as usize)).name);
                    for index in 0..length {
                        let mut at = index;
                        while at > 0 && name(*order.add(at - 1)) > name(index as u32) {
                            *order.add(at) = *order.add(at - 1);
                            at -= 1;
                        }
                        *order.add(at) = index as u32;
                    }
                    self.fields((b"{", b"}"), true, record.fields, Some(order), record.length, true);
                }
                // The registry's render override: the canonical text form.
                K::DURATION => self.text(duration::text(payload.integer).as_bytes()),
                // An Instant has no text form, so debug shows its offset from the origin.
                K::INSTANT => {
                    self.text(b"Instant(");
                    self.text(duration::text(payload.integer).as_bytes());
                    self.text(b")");
                }
                K::TIMESTAMP => self.text(timestamp::text(payload.integer).as_bytes()),
                // Not the `Decimal.to_text` form, which keeps the scale: equal
                // values render the same.
                K::DECIMAL => {
                    let decimal = super::decimals::of_payload(payload.decimal);
                    self.text(decimal.normalized().text().as_bytes());
                }
                K::RANGE => {
                    self.integer(payload.range.start);
                    self.text(b"..");
                    self.integer(payload.range.end);
                }
                K::TUPLE => {
                    let tuple = payload.record;
                    self.fields((b"(", b")"), false, tuple.fields, None, tuple.length, false);
                }
                K::CONSTRUCTED => {
                    let constructed = payload.constructed;
                    let standard = standard_constructor_of(constructed.constructor);
                    let prelude =
                        standard == StandardConstructor::OPTION_SOME || standard == StandardConstructor::OPTION_NONE;
                    let length = constructed.length;
                    if !prelude {
                        self.text(c_bytes(constructed.type_name));
                    }
                    if constructed.variant_name.is_null() {
                        self.fields((b" {", b"}"), true, constructed.fields, None, length, true);
                        return;
                    }
                    if !prelude {
                        self.text(b".");
                    }
                    self.text(c_bytes(constructed.variant_name));
                    if length > 0 {
                        self.fields((b"(", b")"), false, constructed.fields, None, length, length > 1);
                    }
                }
                K::MAP => self.entries(payload.map, true),
                K::SET => self.entries(payload.map, false),
                K::JSON_NUMBER => {
                    self.text(b"Json.Number(");
                    let canonical = json_number::canonical_text(c_bytes(payload.json_number));
                    self.text(&canonical);
                    self.text(b")");
                }
                K::CALLABLE => {
                    let callable = &*payload.callable;
                    let name = match callable.kind {
                        CallableKind::DECLARED => symbol_name(callable.symbol),
                        CallableKind::DEPENDENCY | CallableKind::INTRINSIC | CallableKind::CONSTRUCTOR => {
                            callable.name
                        }
                        _ => core::ptr::null(),
                    };
                    self.text(b"<fn");
                    if !name.is_null() {
                        self.text(b" ");
                        self.text(c_bytes(name));
                    }
                    self.text(b">");
                }
                K::TASK => self.text(b"<Task>"),
                // Diagnostic data, read through `Cause.to_text`, like a function's body.
                K::CAUSE => self.text(b"<Cause>"),
                K::CHANNEL_SENDER => self.text(b"<Sender>"),
                K::CHANNEL_RECEIVER => self.text(b"<Receiver>"),
                K::CHANNEL_COMPLETION => self.text(b"<Completion>"),
                K::PROCESS_ADDRESS => {
                    let address = payload.address;
                    let label: Option<&[u8]> = match address.kind {
                        ADDRESS_NAMED => {
                            let label = chimi_runtime_process_name_label_v1(address.target);
                            (!label.is_null()).then(|| c_bytes(label))
                        }
                        ADDRESS_TEST => Some(b"test"),
                        _ => None,
                    };
                    self.text(b"<address");
                    if let Some(label) = label {
                        self.text(b" ");
                        self.text(label);
                    }
                    self.text(b">");
                }
                K::PROCESS_INBOX => self.text(b"<Inbox>"),
                K::SUPERVISOR_RUNNING => self.text(b"<Running>"),
                K::MANAGED_PROCESS => self.text(b"<ManagedProcess>"),
                K::MANAGED_SUPERVISOR => self.text(b"<ManagedSupervisor>"),
                K::PROCESS_MONITOR => self.text(b"<Monitor>"),
                K::TEST_RECORDER => self.text(b"<Recorder>"),
                K::TEST_SCRIPT => self.text(b"<Script>"),
                K::TEST_MAILBOX => self.text(b"<Mailbox>"),
                K::NATIVE_HANDLE => {
                    self.text(b"<");
                    self.text(c_bytes(payload.handle.name));
                    self.text(b">");
                }
                _ => {}
            }
        }
    }
}

/// `value` in the debug format, indented when `indented`, as a Text.
///
/// # Safety
/// Runs on a fiber that may allocate; `value` is live.
pub unsafe fn render(value: *const Value, indented: bool) -> *mut Value {
    let mut renderer = Renderer { output: HeapText::new(), steps: Vec::new(), indented, depth: 0 };
    renderer.push(Step::Value(value));
    // SAFETY: the caller's contract; every queued value is part of `value`,
    // which stays live while it renders.
    unsafe {
        while let Some(step) = renderer.steps.pop() {
            match step {
                Step::Value(value) => renderer.expand(value),
                Step::Label(label) => {
                    renderer.text(c_bytes(label));
                    renderer.text(b": ");
                }
                Step::Key(key) => {
                    renderer.push(Step::KeyEnd(renderer.indented));
                    renderer.push(Step::Value(key));
                    renderer.indented = false;
                }
                Step::KeyEnd(indented) => {
                    renderer.indented = indented;
                    renderer.text(b": ");
                }
                Step::Separator { index, spaced } => renderer.separator(index, spaced),
                Step::PartEnd => {
                    if renderer.indented {
                        renderer.text(b",");
                    }
                }
                Step::Close { close, spaced } => {
                    renderer.depth -= 1;
                    renderer.close(close, spaced);
                }
            }
        }
        let text = renderer.output.bytes();
        text_value(text.as_ptr().cast(), text.len())
    }
}
