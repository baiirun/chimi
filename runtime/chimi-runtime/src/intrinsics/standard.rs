//! The program's symbol and constructor tables, which the compiler emits as
//! C data, and the standard values built from them: `Option`, and the
//! controls every intrinsic returns. `runtime.c` keeps its own
//! `standard_value` and `intrinsic_shape_defect` for the intrinsics still in
//! C, so a change to either belongs in both until those move.

use core::ffi::{CStr, c_char};
use core::ptr;

use crate::heap::layout::{
    AllocationKind, Constructor, Control, ControlKind, Field, StandardConstructor, Symbol, Value, ValueKind,
};
use crate::heap::values::{allocate_kind, new_control, new_value};

unsafe extern "C" {
    static CHIMI_STRINGS: [*const c_char; 0];
    static CHIMI_IDS: [u32; 0];
    static CHIMI_SYMBOLS: [Symbol; 0];
    static CHIMI_SYMBOL_COUNT: u32;
    static CHIMI_CONSTRUCTORS: [Constructor; 0];
    /// The shared Void (0), False (1) or True (2) in the static area.
    fn chimi_runtime_shared_value_v1(index: u32) -> *mut Value;
}

/// The program's string `id`.
///
/// # Safety
/// `id` is a string id of the program.
unsafe fn string(id: u32) -> *const c_char {
    // SAFETY: the caller's contract; the table holds every string id.
    unsafe { *(&raw const CHIMI_STRINGS).cast::<*const c_char>().add(id as usize) }
}

/// The program's symbol `index`.
///
/// # Safety
/// `index` is below `CHIMI_SYMBOL_COUNT`.
unsafe fn symbol(index: u32) -> &'static Symbol {
    // SAFETY: the caller's contract.
    unsafe { &*(&raw const CHIMI_SYMBOLS).cast::<Symbol>().add(index as usize) }
}

/// The standard constructor id of the program's symbol `index`, as
/// `ChimiSymbol.standard_constructor` holds it.
///
/// # Safety
/// `index` is a symbol of the program.
pub unsafe fn standard_constructor_of(index: u32) -> u32 {
    // SAFETY: the caller's contract.
    unsafe { symbol(index) }.standard_constructor
}

/// The symbol of the standard constructor `standard`, if the program has one.
fn standard_constructor_symbol(standard: u32) -> Option<u32> {
    // SAFETY: the program defines its symbol count.
    let count = unsafe { CHIMI_SYMBOL_COUNT };
    // SAFETY: each index is below the count.
    (0..count).find(|&index| unsafe { symbol(index) }.standard_constructor == standard)
}

/// A value of the standard constructor `standard` holding `values`, or
/// null when the program has no such constructor or it takes another
/// number of fields.
///
/// # Safety
/// Runs on a fiber that may allocate; `values` holds `values.len()` live values.
pub unsafe fn standard_value(standard: u32, values: &[*mut Value]) -> *mut Value {
    let Some(index) = standard_constructor_symbol(standard) else { return ptr::null_mut() };
    // SAFETY: the program's tables are consistent: a constructor symbol names
    // a constructor, whose names and field names are string ids.
    unsafe {
        let shape = &*(&raw const CHIMI_CONSTRUCTORS).cast::<Constructor>().add(symbol(index).constructor as usize);
        if shape.fields_length as usize != values.len() {
            return ptr::null_mut();
        }
        let value = new_value(ValueKind::CONSTRUCTED);
        let constructed = &mut (*value).payload.constructed;
        constructed.constructor = index;
        constructed.type_name = string(shape.type_name);
        constructed.variant_name = if shape.variant_name == u32::MAX { ptr::null() } else { string(shape.variant_name) };
        constructed.length = values.len() as u32;
        let fields = allocate_kind::<Field>(values.len(), AllocationKind::FIELDS);
        constructed.fields = fields;
        let ids = (&raw const CHIMI_IDS).cast::<u32>();
        for (field, &item) in values.iter().enumerate() {
            *fields.add(field) = Field { name: string(*ids.add(shape.fields_offset as usize + field)), value: item };
        }
        value
    }
}

/// The shared Void.
///
/// # Safety
/// Runs on a fiber.
pub unsafe fn void_value() -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { chimi_runtime_shared_value_v1(0) }
}

/// The shared True or False.
///
/// # Safety
/// Runs on a fiber.
pub unsafe fn boolean_value(raw: bool) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { chimi_runtime_shared_value_v1(if raw { 2 } else { 1 }) }
}

/// A successful control holding `value`.
///
/// # Safety
/// Runs on a fiber that may allocate.
pub unsafe fn success(value: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { new_control(ControlKind::SUCCESS, value, ptr::null(), -1) }
}

/// A failed control holding `value`.
///
/// # Safety
/// Runs on a fiber that may allocate.
pub unsafe fn failure(value: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { new_control(ControlKind::FAILURE, value, ptr::null(), -1) }
}

/// A defect at `node` with a static message.
///
/// # Safety
/// Runs on a fiber that may allocate.
pub unsafe fn defect(message: &'static CStr, node: i64) -> *mut Control {
    // SAFETY: the caller's contract; the message outlives the run.
    unsafe { new_control(ControlKind::DEFECT, ptr::null_mut(), message.as_ptr(), node) }
}

/// The defect of a standard function the compiler verified but whose
/// arguments the runtime does not accept.
///
/// # Safety
/// Runs on a fiber that may allocate.
pub unsafe fn shape_defect(call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { defect(c"verified standard function has invalid runtime arguments", i64::from(call_node)) }
}

/// Success with `Some(found)`, or `None` for null.
///
/// # Safety
/// Runs on a fiber that may allocate; `found` is null or live.
pub unsafe fn option_value(found: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if found.is_null() {
            return success(standard_value(StandardConstructor::OPTION_NONE, &[]));
        }
        success(standard_value(StandardConstructor::OPTION_SOME, &[found]))
    }
}
