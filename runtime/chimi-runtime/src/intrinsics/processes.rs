//! The data constructors of std/process and std/supervisor
//! (docs/architecture/supervised-process-runtime.md, "Mapping onto the
//! native runtime"): `Process.spec`, `Process.handoff_spec`,
//! `Process.register`, and `Supervisor.process`, `supervisor`, `policy` and
//! `restarts`. Moved from `runtime.c`; the operations that talk to the
//! scheduler are still there.
//!
//! Specs, members, policies and restart limits are plain records with the
//! field names the interpreter uses, so module constants built at compile
//! time and values built here are the same. Their fields are in ascending
//! name order. The range defects match the interpreter's
//! (process-and-supervisor-static-contracts.md).

use core::ffi::{CStr, c_char};

use super::message::Message;
use super::standard::{boolean_value, defect, defect_with, shape_defect, standard_value, success};
use crate::duration;
use crate::eval::select_value;
use crate::heap::layout::{AllocationKind, Control, Field, Intrinsic, StandardConstructor, Value, ValueKind};
use crate::heap::values::{allocate_kind, new_value};

/// A record of `fields`, whose names are in ascending order.
///
/// # Safety
/// Runs on a fiber that may allocate; each value is live.
unsafe fn record(fields: &[(&'static CStr, *mut Value)]) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::RECORD);
        (*value).payload.record.length = fields.len() as u32;
        let slots = allocate_kind::<Field>(fields.len(), AllocationKind::FIELDS);
        (*value).payload.record.fields = slots;
        for (index, &(name, field)) in fields.iter().enumerate() {
            *slots.add(index) = Field { name: name.as_ptr(), value: field };
        }
        value
    }
}

/// The field `name` of a record, or null.
///
/// # Safety
/// `value` is live.
unsafe fn field(value: *mut Value, name: &'static CStr) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        if (*value).kind != ValueKind::RECORD {
            return core::ptr::null_mut();
        }
        select_value(value, name.as_ptr())
    }
}

/// `<heading><value>`, as a defect.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn range_defect(heading: &[u8], value: i64, node: i64) -> *mut Control {
    let message: *const c_char = Message::new().text(heading).integer(value).finish();
    // SAFETY: the caller's contract.
    unsafe { defect_with(message, node) }
}

/// `<heading><duration text>`, as a defect.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn duration_defect(heading: &[u8], nanoseconds: i64, node: i64) -> *mut Control {
    let text = duration::text(nanoseconds);
    let message: *const c_char = Message::new().text(heading).text(text.as_bytes()).finish();
    // SAFETY: the caller's contract.
    unsafe { defect_with(message, node) }
}

/// The intrinsic `intrinsic`, a process or supervisor data constructor, on
/// `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn process_data_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let shape = |kinds: &[ValueKind]| {
            arguments.len() == kinds.len() && arguments.iter().zip(kinds).all(|(&value, &kind)| (*value).kind == kind)
        };
        let integer = |index: usize| (*arguments[index]).payload.integer;
        let spec_shape = [ValueKind::CALLABLE, ValueKind::INTEGER, ValueKind::DURATION];
        match intrinsic {
            Intrinsic::PROCESS_SPEC if shape(&spec_shape) => {
                if integer(1) < 1 {
                    return range_defect(b"Process.spec: mailbox capacity must be at least 1, got ", integer(1), node);
                }
                if integer(2) < 0 {
                    return duration_defect(
                        b"Process.spec: shutdown_within must be at least zero, got ",
                        integer(2),
                        node,
                    );
                }
                let none = standard_value(StandardConstructor::OPTION_NONE, &[]);
                if none.is_null() {
                    return shape_defect(call_node);
                }
                success(record(&[
                    (c"body", arguments[0]),
                    (c"capacity", arguments[1]),
                    (c"name", none),
                    (c"shutdown_within", arguments[2]),
                ]))
            }
            // A spec with `handoff` set, as the interpreter builds it. It has
            // no name: `Process.register` takes only a `Process.Spec`.
            Intrinsic::PROCESS_HANDOFF_SPEC if shape(&spec_shape) => {
                if integer(1) < 1 {
                    return range_defect(
                        b"Process.handoff_spec: mailbox capacity must be at least 1, got ",
                        integer(1),
                        node,
                    );
                }
                if integer(2) < 0 {
                    return duration_defect(
                        b"Process.handoff_spec: shutdown_within must be at least zero, got ",
                        integer(2),
                        node,
                    );
                }
                let handoff = boolean_value(true);
                let none = standard_value(StandardConstructor::OPTION_NONE, &[]);
                if none.is_null() {
                    return shape_defect(call_node);
                }
                success(record(&[
                    (c"body", arguments[0]),
                    (c"capacity", arguments[1]),
                    (c"handoff", handoff),
                    (c"name", none),
                    (c"shutdown_within", arguments[2]),
                ]))
            }
            Intrinsic::PROCESS_REGISTER
                if arguments.len() == 2 && (*arguments[1]).kind == ValueKind::PROCESS_ADDRESS =>
            {
                let spec = arguments[0];
                let name = field(spec, c"name");
                if name.is_null() || (*name).kind != ValueKind::CONSTRUCTED {
                    return shape_defect(call_node);
                }
                if (*name).payload.constructed.length != 0 {
                    return defect(c"Process.register: spec already has a name", node);
                }
                let body = field(spec, c"body");
                let capacity = field(spec, c"capacity");
                let some = standard_value(StandardConstructor::OPTION_SOME, &[arguments[1]]);
                let within = field(spec, c"shutdown_within");
                if body.is_null() || capacity.is_null() || some.is_null() || within.is_null() {
                    return shape_defect(call_node);
                }
                success(record(&[
                    (c"body", body),
                    (c"capacity", capacity),
                    (c"name", some),
                    (c"shutdown_within", within),
                ]))
            }
            Intrinsic::SUPERVISOR_PROCESS if arguments.len() == 2 => {
                success(record(&[(c"restart", arguments[1]), (c"spec", arguments[0])]))
            }
            Intrinsic::SUPERVISOR_SUPERVISOR if arguments.len() == 2 => {
                success(record(&[(c"policy", arguments[0]), (c"restart", arguments[1])]))
            }
            Intrinsic::SUPERVISOR_POLICY
                if arguments.len() == 3
                    && (*arguments[0]).kind == ValueKind::LIST
                    && (*arguments[1]).kind == ValueKind::INTEGER =>
            {
                let capacity = integer(1);
                if capacity < 1 {
                    return range_defect(b"Supervisor.policy: capacity must be at least 1, got ", capacity, node);
                }
                let members = (*arguments[0]).payload.list.length;
                if i64::from(members) > capacity {
                    let message: *const c_char = Message::new()
                        .text(b"Supervisor.policy: members must not exceed capacity ")
                        .integer(capacity)
                        .text(b", got ")
                        .integer(i64::from(members))
                        .finish();
                    return defect_with(message, node);
                }
                success(record(&[
                    (c"capacity", arguments[1]),
                    (c"members", arguments[0]),
                    (c"restarts", arguments[2]),
                ]))
            }
            Intrinsic::SUPERVISOR_RESTARTS if shape(&[ValueKind::INTEGER, ValueKind::DURATION]) => {
                if integer(0) < 0 {
                    return range_defect(
                        b"Supervisor.restarts: restart limit must be at least 0, got ",
                        integer(0),
                        node,
                    );
                }
                if integer(1) <= 0 {
                    return duration_defect(
                        b"Supervisor.restarts: restart window must be positive, got ",
                        integer(1),
                        node,
                    );
                }
                success(record(&[(c"limit", arguments[0]), (c"window", arguments[1])]))
            }
            _ => shape_defect(call_node),
        }
    }
}
