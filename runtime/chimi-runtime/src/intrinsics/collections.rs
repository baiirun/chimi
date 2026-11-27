//! The native cores of std/map and std/set (a Set is a Map whose values are
//! Void); `execution/map_operations.rs` is the interpreter's.

use core::ffi::c_void;

use super::standard::{boolean_value, option_value, shape_defect, success, void_value};
use crate::heap::layout::{Callable, Control, ControlKind, Environment, Intrinsic, MapNode, Value, ValueKind};
use crate::heap::map;
use crate::heap::values::{integer_value, pointer_value};

/// One intrinsic call's context, passed through to callbacks.
pub struct Call {
    pub runtime: *mut c_void,
    pub span: u64,
    pub call_node: u32,
    pub environment: *mut Environment,
}

/// Calls `step` with the state, starting at `initial`, then the key of each
/// node of `root` in order and, when `with_value`, its value; stops at the
/// first call that does not succeed.
///
/// # Safety
/// Runs on a fiber; `root` is null or a live tree; `step` is live.
unsafe fn reduce_in_order(
    call: &Call,
    step: *mut Callable,
    initial: *mut Value,
    root: *const MapNode,
    with_value: bool,
) -> *mut Control {
    // SAFETY: the caller's contract; the tree is immutable, so its nodes stay
    // valid while the callbacks run, as in the C.
    unsafe {
        let entries = map::entries(root);
        let mut control = success(initial);
        for &node in &entries {
            let mut arguments = [(*control).value, (*node).key, (*node).value];
            control = crate::eval::invoke(
                call.runtime.cast(),
                step,
                arguments.as_mut_ptr(),
                if with_value { 3 } else { 2 },
                call.span,
                call.call_node,
                call.environment,
            );
            if (*control).kind != ControlKind::SUCCESS {
                break;
            }
        }
        control
    }
}

/// The callable in `value`, if it is one.
///
/// # Safety
/// `value` is live.
unsafe fn callable(value: *mut Value) -> Option<*mut Callable> {
    // SAFETY: the caller's contract; the payload is read as the kind says.
    unsafe { ((*value).kind == ValueKind::CALLABLE).then(|| (*value).payload.callable) }
}

/// std/map's intrinsic `intrinsic` on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn map_operation(call: &Call, intrinsic: u32, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        if intrinsic == Intrinsic::MAP_EMPTY {
            return if arguments.is_empty() {
                success(pointer_value(ValueKind::MAP, core::ptr::null_mut()))
            } else {
                shape_defect(call.call_node)
            };
        }
        let Some(&first) = arguments.first() else { return shape_defect(call.call_node) };
        if (*first).kind != ValueKind::MAP {
            return shape_defect(call.call_node);
        }
        let tree = (*first).payload.map;
        match (intrinsic, arguments.len()) {
            (Intrinsic::MAP_GET, 2) => option_value(map::get(tree, arguments[1])),
            (Intrinsic::MAP_PUT, 3) => success(pointer_value(ValueKind::MAP, map::put(tree, arguments[1], arguments[2]).cast())),
            (Intrinsic::MAP_REMOVE, 2) => {
                let removed = map::remove(tree, arguments[1]);
                success(if removed == tree { first } else { pointer_value(ValueKind::MAP, removed.cast()) })
            }
            (Intrinsic::MAP_SIZE, 1) => success(integer_value(ValueKind::INTEGER, map::size(tree))),
            (Intrinsic::MAP_REDUCE_ENTRIES, 3) => match callable(arguments[2]) {
                Some(step) => reduce_in_order(call, step, arguments[1], tree, true),
                None => shape_defect(call.call_node),
            },
            _ => shape_defect(call.call_node),
        }
    }
}

/// std/set's intrinsic `intrinsic` on `arguments`.
///
/// # Safety
/// As for [`map_operation`].
pub unsafe fn set_operation(call: &Call, intrinsic: u32, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        if intrinsic == Intrinsic::SET_EMPTY {
            return if arguments.is_empty() {
                success(pointer_value(ValueKind::SET, core::ptr::null_mut()))
            } else {
                shape_defect(call.call_node)
            };
        }
        let Some(&first) = arguments.first() else { return shape_defect(call.call_node) };
        if (*first).kind != ValueKind::SET {
            return shape_defect(call.call_node);
        }
        let tree = (*first).payload.map;
        match (intrinsic, arguments.len()) {
            (Intrinsic::SET_CONTAINS, 2) => success(boolean_value(!map::get(tree, arguments[1]).is_null())),
            (Intrinsic::SET_ADD, 2) => {
                success(pointer_value(ValueKind::SET, map::put(tree, arguments[1], void_value()).cast()))
            }
            (Intrinsic::SET_REMOVE, 2) => {
                let removed = map::remove(tree, arguments[1]);
                success(if removed == tree { first } else { pointer_value(ValueKind::SET, removed.cast()) })
            }
            (Intrinsic::SET_SIZE, 1) => success(integer_value(ValueKind::INTEGER, map::size(tree))),
            (Intrinsic::SET_REDUCE_ITEMS, 3) => match callable(arguments[2]) {
                Some(step) => reduce_in_order(call, step, arguments[1], tree, false),
                None => shape_defect(call.call_node),
            },
            _ => shape_defect(call.call_node),
        }
    }
}
