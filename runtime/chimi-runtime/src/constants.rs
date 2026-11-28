//! Module constants (CHI-305 step 5, phase 8): decoded from the program's
//! `CHIMI_CONSTANT_VALUES` before the entry runs. They are allocated in the
//! static area, so no collection frees them, and values are immutable, so
//! every task shares them. Decoding cannot fail and emits no events.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use crate::eval::{callable_value, construct_value, find_function, slot, string_bytes, string_text};
use crate::heap::allocator::{allocate, current_carrier};
use crate::heap::fragment::set_kind;
use crate::heap::layout::{
    AllocationArea, AllocationKind, CallableKind, Decimal, Environment, Field, MapNode, Node, Trace, Value, ValueKind,
};
use crate::heap::list;
use crate::heap::map;
use crate::heap::values::{
    allocate_kind, capture_requirements, decimal_value, float_value, integer_value, json_number_value,
    new_environment, new_value, pointer_value, range_value,
};
use crate::intrinsics::standard::{string, symbol, void_value};

/// `ChimiConstantValue` (runtime.h): one constant's encoding; `kind` says
/// what the other fields hold.
#[repr(C)]
pub struct ConstantValue {
    pub kind: u32,
    pub a: u32,
    pub offset: u32,
    pub length: u32,
    pub integer: i64,
}

/// The `CHIMI_CONSTANT_*` kinds (runtime.h).
pub struct ConstantKind;

impl ConstantKind {
    pub const VOID: u32 = 0;
    pub const INTEGER: u32 = 1;
    pub const FLOAT: u32 = 2;
    pub const TEXT: u32 = 3;
    pub const BOOLEAN: u32 = 4;
    pub const LIST: u32 = 5;
    pub const RECORD: u32 = 6;
    pub const CONSTRUCTED: u32 = 7;
    pub const MAP: u32 = 8;
    pub const JSON_NUMBER: u32 = 9;
    pub const DECLARED: u32 = 10;
    pub const INTRINSIC: u32 = 11;
    pub const CONSTRUCTOR: u32 = 12;
    pub const CLOSURE: u32 = 13;
    pub const TUPLE: u32 = 14;
    pub const RANGE: u32 = 15;
    pub const DURATION: u32 = 16;
    pub const PROCESS_ADDRESS: u32 = 17;
    pub const DECIMAL: u32 = 18;
    pub const SET: u32 = 19;
    pub const BYTES: u32 = 20;
}

/// The decoded module constants, by value id; the evaluator reads them.
#[unsafe(no_mangle)]
pub static chimi_runtime_constant_values: AtomicPtr<*mut Value> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" {
    static CHIMI_CONSTANT_VALUES: [ConstantValue; 0];
    static CHIMI_CONSTANT_VALUE_COUNT: u32;
    static CHIMI_IDS: [u32; 0];
    static CHIMI_NODES: [Node; 0];
    /// The area static values live in (runtime.c).
    static mut chimi_runtime_static_area: AllocationArea;
    /// The shared Void, False and True, made on first use (runtime.c).
    fn chimi_runtime_shared_value_v1(index: u32) -> *mut Value;
    fn chimi_runtime_bind_requirement_v1(environment: *mut Environment, name: *const core::ffi::c_char, value: *mut Value);
}

/// The ids at `offset` in the program's id table.
///
/// # Safety
/// The table holds them.
unsafe fn ids(offset: u32) -> *const u32 {
    // SAFETY: the caller's contract.
    unsafe { (&raw const CHIMI_IDS).cast::<u32>().add(offset as usize) }
}

/// # Safety
/// `constants` holds every value id below the one being decoded.
unsafe fn constant(constants: *mut *mut Value, id: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { *constants.add(id as usize) }
}

/// Binds each (requirement name, value id) pair of `count` at `pairs` to
/// every requirement of that name, as a provision does.
///
/// # Safety
/// As for [`decode`].
unsafe fn bind_requirements(constants: *mut *mut Value, environment: *mut Environment, pairs: *const u32, count: i64) {
    // SAFETY: the caller's contract.
    unsafe {
        for index in 0..count as usize {
            chimi_runtime_bind_requirement_v1(
                environment,
                string(*pairs.add(2 * index)),
                constant(constants, *pairs.add(2 * index + 1)),
            );
        }
    }
}

fn hex_value(digit: u8) -> u8 {
    if digit <= b'9' { digit.wrapping_sub(b'0') } else { digit.wrapping_sub(b'a').wrapping_add(10) }
}

/// The Bytes a constant's lowercase hex text spells, two digits per byte.
///
/// # Safety
/// Runs with an allocation target.
unsafe fn bytes_from_hex(hex: &[u8]) -> *mut Value {
    let length = hex.len() / 2;
    // SAFETY: the caller's contract; the buffer holds `length + 1` bytes.
    unsafe {
        let raw = allocate(length + 1, 1);
        for index in 0..length {
            *raw.add(index) = hex_value(hex[2 * index]) << 4 | hex_value(hex[2 * index + 1]);
        }
        *raw.add(length) = 0;
        let value = new_value(ValueKind::BYTES);
        (*value).payload.text.bytes = raw.cast();
        (*value).payload.text.length = length;
        value
    }
}

/// `length` fields at `fields`, named by `name(index)`.
///
/// # Safety
/// Runs with an allocation target.
unsafe fn fields_value(kind: ValueKind, length: u32, field: impl Fn(usize) -> Field) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(kind);
        (*value).payload.record.length = length;
        let fields = allocate_kind::<Field>(length as usize, AllocationKind::FIELDS);
        (*value).payload.record.fields = fields;
        for index in 0..length as usize {
            fields.add(index).write(field(index));
        }
        value
    }
}

/// The constant `raw` encodes.
///
/// # Safety
/// Runs with the static area as the allocation target; `constants` holds
/// every value id `raw` names, which the compiler emits first.
unsafe fn decode(constants: *mut *mut Value, raw: &ConstantValue) -> *mut Value {
    // SAFETY: the caller's contract; the compiler emitted a consistent table.
    unsafe {
        let ids = ids(raw.offset);
        let id = |index: usize| *ids.add(index);
        let of = |index: usize| constant(constants, id(index));
        match raw.kind {
            ConstantKind::VOID => return void_value(),
            ConstantKind::INTEGER => return integer_value(ValueKind::INTEGER, raw.integer),
            ConstantKind::FLOAT => return float_value(f64::from_bits(raw.integer as u64)),
            ConstantKind::TEXT => return string_text(raw.a),
            ConstantKind::BYTES => return bytes_from_hex(string_bytes(raw.a)),
            ConstantKind::BOOLEAN => return chimi_runtime_shared_value_v1(if raw.integer != 0 { 2 } else { 1 }),
            ConstantKind::JSON_NUMBER => return json_number_value(string(raw.a)),
            ConstantKind::LIST => {
                let items = allocate(raw.length as usize, size_of::<*mut Value>()).cast::<*mut Value>();
                for index in 0..raw.length as usize {
                    *items.add(index) = of(index);
                }
                return list::list_value(list::from_values(items, raw.length), 0, raw.length);
            }
            ConstantKind::DURATION => return integer_value(ValueKind::DURATION, raw.integer),
            ConstantKind::DECIMAL => {
                let high = (u64::from(raw.offset) << 32) | u64::from(raw.length);
                return decimal_value(Decimal {
                    high,
                    low: raw.integer as u64,
                    scale: raw.a & 0xff,
                    negative: raw.a & 0x100 != 0,
                });
            }
            ConstantKind::PROCESS_ADDRESS => {
                let value = new_value(ValueKind::PROCESS_ADDRESS);
                (*value).payload.address.kind = Trace::ADDRESS_NAMED;
                (*value).payload.address.target = u64::from(raw.a) + 1;
                return value;
            }
            ConstantKind::RANGE => return range_value((*of(0)).payload.integer, (*of(1)).payload.integer),
            ConstantKind::TUPLE => {
                return fields_value(ValueKind::TUPLE, raw.length, |index| Field { name: ptr::null(), value: of(index) });
            }
            ConstantKind::RECORD => {
                return fields_value(ValueKind::RECORD, raw.length, |index| Field {
                    name: string(id(2 * index)),
                    value: of(2 * index + 1),
                });
            }
            ConstantKind::CONSTRUCTED => {
                let fields = allocate(raw.length as usize, size_of::<*mut Value>()).cast::<*mut Value>();
                for index in 0..raw.length as usize {
                    *fields.add(index) = of(index);
                }
                return (*construct_value(raw.a, fields)).value;
            }
            ConstantKind::MAP => {
                let mut node: *mut MapNode = ptr::null_mut();
                for index in 0..raw.length as usize {
                    node = map::put(node, of(2 * index), of(2 * index + 1));
                }
                return pointer_value(ValueKind::MAP, node.cast());
            }
            ConstantKind::SET => {
                let mut node: *mut MapNode = ptr::null_mut();
                for index in 0..raw.length as usize {
                    node = map::put(node, of(index), void_value());
                }
                return pointer_value(ValueKind::SET, node.cast());
            }
            _ => {}
        }
        let callable = allocate_kind::<crate::heap::layout::Callable>(1, AllocationKind::CALLABLE);
        let shape = symbol(raw.a);
        match raw.kind {
            ConstantKind::DECLARED => {
                let function = find_function(raw.a);
                (*callable).kind = CallableKind::DECLARED;
                (*callable).symbol = raw.a;
                (*callable).body = (*function).body;
                (*callable).parameters_offset = (*function).parameters_offset;
                (*callable).parameters_length = (*function).parameters_length;
                (*callable).local_count = (*function).local_count;
                (*callable).environment = capture_requirements(new_environment(0));
                (*callable).name = c"function".as_ptr();
                bind_requirements(constants, (*callable).environment, ids, raw.integer);
            }
            ConstantKind::INTRINSIC => {
                (*callable).kind = CallableKind::INTRINSIC;
                (*callable).intrinsic = shape.intrinsic;
                (*callable).symbol = raw.a;
                (*callable).name = string(shape.name);
            }
            ConstantKind::CONSTRUCTOR => {
                (*callable).kind = CallableKind::CONSTRUCTOR;
                (*callable).symbol = raw.a;
                (*callable).name = string(shape.name);
            }
            ConstantKind::CLOSURE => {
                let node = &*(&raw const CHIMI_NODES).cast::<Node>().add(raw.a as usize);
                let environment = new_environment(node.d);
                set_kind(environment.cast::<c_void>(), AllocationKind::ENVIRONMENT);
                for index in 0..raw.length as usize {
                    *(*environment).locals.add(slot(id(2 * index))) = of(2 * index + 1);
                }
                bind_requirements(constants, environment, ids.add(2 * raw.length as usize), raw.integer);
                (*callable).kind = CallableKind::CLOSURE;
                (*callable).body = node.a;
                (*callable).parameters_offset = node.offset;
                (*callable).parameters_length = node.length;
                (*callable).environment = environment;
                (*callable).name = c"closure".as_ptr();
            }
            _ => libc::abort(),
        }
        callable_value(callable)
    }
}

/// Decodes every module constant into the static area, after making the
/// shared Void, False and True there.
///
/// # Safety
/// Runs once, on the main thread, before the entry and before any carrier
/// thread starts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_decode_constants_v1() {
    // SAFETY: the caller's contract.
    unsafe {
        for index in 0..3 {
            chimi_runtime_shared_value_v1(index);
        }
        let carrier = current_carrier();
        let previous = (*carrier).fiber.target;
        (*carrier).fiber.target = &raw mut chimi_runtime_static_area;
        let count = CHIMI_CONSTANT_VALUE_COUNT;
        let constants = allocate(count as usize, size_of::<*mut Value>()).cast::<*mut Value>();
        chimi_runtime_constant_values.store(constants, Relaxed);
        let table = (&raw const CHIMI_CONSTANT_VALUES).cast::<ConstantValue>();
        for index in 0..count as usize {
            *constants.add(index) = decode(constants, &*table.add(index));
        }
        (*carrier).fiber.target = previous;
    }
}
