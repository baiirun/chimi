//! `#[repr(C)]` mirrors of `runtime.c`'s heap and value structs (CHI-305
//! step 5, phase 0), so the Rust allocator and collector of the later phases
//! read and write the same memory as the C that is still there.
//!
//! [`LAYOUT`] lists the numbers the mirrors depend on, measured here: each
//! enum constant, each struct's size (and the alignment of those whose
//! alignment matters), and each field's offset, size and, for a scalar,
//! its class (signed or unsigned integer, `bool`, floating point).
//! `runtime.c` carries the same numbers as `_Static_assert`s in a generated
//! block ([`c_asserts`]), and the compiler's `runtime_layout` test fails when
//! that block differs from this table, so a Rust change shows up in the test
//! and a C change that moves a listed number fails the C compile.
//!
//! Numbers cannot see everything: a constant appended to a C enum, or a field
//! added in a struct's padding, moves none of them. So the same test also
//! keeps a digest of each mirrored C definition and fails when one changes,
//! which sends whoever changed it here to update the mirror.
//!
//! The tags are newtypes over `u32` rather than Rust enums, because C may
//! hold any value in them. The carrier's state (`Carrier`) is mirrored since
//! the allocator moved (phase 1); the fragment copier's (`FragmentCopy`)
//! is mirrored with the phase that moves its code.

use core::ffi::{c_char, c_void};
use core::mem::{align_of, offset_of, size_of};

/// `ValueKind`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueKind(pub u32);

impl ValueKind {
    pub const VOID: Self = Self(0);
    pub const INTEGER: Self = Self(1);
    pub const FLOAT: Self = Self(2);
    pub const TEXT: Self = Self(3);
    pub const BOOLEAN: Self = Self(4);
    pub const LIST: Self = Self(5);
    pub const RECORD: Self = Self(6);
    pub const CONSTRUCTED: Self = Self(7);
    pub const CALLABLE: Self = Self(8);
    pub const MAP: Self = Self(9);
    pub const JSON_NUMBER: Self = Self(10);
    pub const TASK: Self = Self(11);
    pub const CHANNEL_SENDER: Self = Self(12);
    pub const CHANNEL_RECEIVER: Self = Self(13);
    pub const CHANNEL_COMPLETION: Self = Self(14);
    pub const TUPLE: Self = Self(15);
    pub const RANGE: Self = Self(16);
    pub const DURATION: Self = Self(17);
    pub const INSTANT: Self = Self(18);
    pub const TIMESTAMP: Self = Self(19);
    pub const DECIMAL: Self = Self(20);
    pub const PROCESS_ADDRESS: Self = Self(21);
    pub const PROCESS_INBOX: Self = Self(22);
    pub const SUPERVISOR_RUNNING: Self = Self(23);
    pub const MANAGED_PROCESS: Self = Self(24);
    pub const MANAGED_SUPERVISOR: Self = Self(25);
    pub const PROCESS_MONITOR: Self = Self(26);
    pub const SET: Self = Self(27);
    pub const BYTES: Self = Self(28);
    pub const CAUSE: Self = Self(29);
    pub const TEST_RECORDER: Self = Self(30);
    pub const TEST_SCRIPT: Self = Self(31);
    pub const TEST_MAILBOX: Self = Self(32);
    pub const NATIVE_HANDLE: Self = Self(33);
}

/// `CallableKind`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallableKind(pub u32);

impl CallableKind {
    pub const DECLARED: Self = Self(0);
    pub const CLOSURE: Self = Self(1);
    pub const DEPENDENCY: Self = Self(2);
    pub const HOST: Self = Self(3);
    pub const INTRINSIC: Self = Self(4);
    pub const CONSTRUCTOR: Self = Self(5);
}

/// `ControlKind`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlKind(pub u32);

impl ControlKind {
    pub const SUCCESS: Self = Self(0);
    pub const FAILURE: Self = Self(1);
    pub const DEFECT: Self = Self(2);
    pub const CANCEL: Self = Self(3);
    pub const BREAK: Self = Self(4);
}

/// `AllocationKind`: what an allocation's payload is, so a walk knows its
/// pointers.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocationKind(pub u32);

impl AllocationKind {
    pub const BYTES: Self = Self(0);
    pub const VALUE: Self = Self(1);
    pub const CONTROL: Self = Self(2);
    pub const ENVIRONMENT: Self = Self(3);
    pub const VALUE_POINTERS: Self = Self(4);
    pub const FIELDS: Self = Self(5);
    pub const CALLABLE: Self = Self(6);
    pub const MAP_NODE: Self = Self(7);
    pub const LIST_NODE: Self = Self(8);
    pub const FRAME_ENVIRONMENT: Self = Self(9);
    pub const COUNT: usize = 10;
}

/// `AreaRole`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AreaRole(pub u32);

impl AreaRole {
    pub const YOUNG: Self = Self(0);
    pub const SURVIVOR: Self = Self(1);
    pub const BASE: Self = Self(2);
    pub const FRAGMENT: Self = Self(3);
    pub const STATIC: Self = Self(4);
}

/// `HeapStatus`: what a collection or copy reports.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapStatus(pub u32);

impl HeapStatus {
    pub const OK: Self = Self(0);
    pub const FOREIGN_POINTER: Self = Self(1);
}

/// `ChimiDecimal`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Decimal {
    pub high: u64,
    pub low: u64,
    pub scale: u32,
    pub negative: bool,
}

/// `RuntimeField`.
#[repr(C)]
pub struct Field {
    pub name: *const c_char,
    pub value: *mut Value,
}

/// `RuntimeCapability`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Capability {
    pub channel: u64,
    pub owner: u64,
    pub token: u64,
}

/// `ListNode`: `slots` (values at height 0, child nodes above) follow it.
#[repr(C)]
pub struct ListNode {
    pub height: u32,
    pub count: u32,
    pub slots: [*mut c_void; 0],
}

/// `ListVector`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ListVector {
    pub root: *mut ListNode,
    pub tail: *mut ListNode,
    pub length: u32,
}

/// `ChimiOrigin` (runtime.h): where a node came from in the source.
#[repr(C)]
pub struct Origin {
    pub file_id: u32,
    pub semantic_id: u32,
    pub start: u32,
    pub end: u32,
    pub location: u32,
}

/// `ChimiNode` (runtime.h): one node of the program's core tree.
#[repr(C)]
pub struct Node {
    pub origin: u32,
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
    pub offset: u32,
    pub length: u32,
    pub integer: i64,
}

/// `ChimiInjectedSignal` (chimi_fiber.h): a std/signal signal the virtual
/// clock latches at `at` nanoseconds.
#[repr(C)]
pub struct InjectedSignal {
    pub code: u32,
    pub at: i64,
}

/// `CHIMI_MAX_INJECTED_SIGNALS`.
pub const MAX_INJECTED_SIGNALS: usize = 64;

/// `struct ChimiRuntime`: one run's settings and its telemetry state. Rust
/// reads it to write telemetry events; `runtime.c` builds it.
#[repr(C)]
pub struct Runtime {
    pub execution_id: *const c_char,
    pub computation: *const c_char,
    pub full_trace: bool,
    pub virtual_clock: bool,
    pub carriers: u32,
    pub clock_start: i64,
    pub start_unix_ns: i64,
    pub deadlock_waiters: *mut c_char,
    pub last_defect_incarnation: u64,
    pub last_defect_membership: u64,
    pub last_defect_message: *mut c_char,
    pub process_defect_count: u64,
    pub capability_violations: *mut c_void,
    /// `volatile sig_atomic_t`.
    pub cancel: i32,
    pub injected_signal: i32,
    pub injected_signal_at: i64,
    pub injected_user_signals: [InjectedSignal; MAX_INJECTED_SIGNALS],
    pub injected_user_signal_count: usize,
    pub silent: bool,
    /// `FILE *`.
    pub telemetry: *mut c_void,
    pub outcome: *mut c_void,
    pub max_line_bytes: u32,
    pub log_level: i64,
    pub log_file: *mut c_void,
    pub metrics_file: *mut c_void,
    pub next_sequence: u64,
    /// `_Atomic uint64_t`.
    pub next_span: core::sync::atomic::AtomicU64,
    pub trace_events_constructed: u64,
}

/// `ChimiSchedulerTraceEvent` (chimi_fiber.h): one event the scheduler's
/// trace hook reports.
#[repr(C)]
pub struct SchedulerTraceEvent {
    pub family: u32,
    pub kind: u32,
    pub sequence: u64,
    pub task: u64,
    pub parent: u64,
    pub cause: u64,
    pub channel: u64,
    pub capacity: u64,
    pub queue_depth: u64,
    pub waiter_depth: u64,
    pub discard_count: u64,
    pub supervisor: u64,
    pub membership: u64,
    pub incarnation: u64,
    pub name: u64,
    pub parent_supervisor: u64,
    pub attempts_in_window: u64,
    pub time: i64,
    pub address_kind: u32,
    pub outcome: u32,
}

/// The scheduler trace event codes (chimi_fiber.h) telemetry names.
pub struct Trace;

impl Trace {
    pub const FAMILY_TASK: u32 = 1;
    pub const FAMILY_CHANNEL: u32 = 2;
    pub const FAMILY_PROCESS: u32 = 3;
    pub const FAMILY_CLOCK: u32 = 4;
    pub const PROCESS_ADMISSION_REJECTED: u32 = 2;
    pub const PROCESS_TERMINATED: u32 = 3;
    pub const PROCESS_SEND: u32 = 9;
    pub const PROCESS_CAUSE_DEFECTED: u64 = 3;
    pub const PROCESS_DEFECT_STACK_OVERFLOW: u32 = 1;
    pub const CLOCK_SELECT_COMMITTED: u32 = 8;
    pub const ADDRESS_NAMED: u32 = 1;
    pub const ADDRESS_DIRECT: u32 = 2;
}

/// `struct ChimiBindingCall`: one call of a C binding function. A binding
/// sees it only through the `chimi_binding_*` functions.
#[repr(C)]
pub struct BindingCall {
    pub runtime: *mut c_void,
    pub arguments: *mut *mut Value,
    pub argument_count: u32,
    pub call_node: u32,
    pub held: *mut bool,
}

/// `ChimiSymbol` (runtime.h): one entry of the program's symbol table.
#[repr(C)]
pub struct Symbol {
    pub kind: u32,
    pub name: u32,
    pub function: u32,
    pub constant: u32,
    pub constructor: u32,
    pub intrinsic: u32,
    pub standard_constructor: u32,
    pub standard_type: u32,
    pub environment_slot: u32,
    pub binding: u32,
}

/// `ChimiConstructor` (runtime.h): one constructor's shape.
#[repr(C)]
pub struct Constructor {
    pub type_name: u32,
    pub variant_name: u32,
    pub fields_offset: u32,
    pub fields_length: u32,
    pub outcome_success: i32,
    pub ordinal: u32,
    pub program_exit: u32,
}

/// `ChimiFunction` (runtime.h): a declared function's body and parameters.
#[repr(C)]
pub struct Function {
    pub symbol: u32,
    pub body: u32,
    pub parameters_offset: u32,
    pub parameters_length: u32,
    pub local_count: u32,
}

/// `ChimiNamedNode` (runtime.h): a record field's or a provider's name and node.
#[repr(C)]
pub struct NamedNode {
    pub name: u32,
    pub node: u32,
}

/// `ChimiStep` (runtime.h): one step of a block; kind 0 binds `symbol`.
#[repr(C)]
pub struct Step {
    pub kind: u32,
    pub symbol: u32,
    pub node: u32,
}

/// `ChimiArm` (runtime.h): one arm of a match.
#[repr(C)]
pub struct Arm {
    pub patterns_offset: u32,
    pub patterns_length: u32,
    pub guard: u32,
    pub body: u32,
    pub cause_subject: u32,
    pub cause_local: u32,
}

/// `ChimiPattern` (runtime.h): one pattern of an arm.
#[repr(C)]
pub struct Pattern {
    pub kind: u32,
    pub a: u32,
    pub offset: u32,
    pub length: u32,
    pub integer: i64,
}

/// `ChimiSchedulerRequest` (chimi_fiber.h): a request a fiber makes of the
/// scheduler, answered in place.
#[repr(C)]
pub struct SchedulerRequest {
    pub kind: u32,
    pub status: u32,
    pub task: u64,
    pub channel: u64,
    pub owner: u64,
    pub token: u64,
    pub value: i64,
    pub entry: *const c_void,
    pub context: *mut c_void,
}

impl SchedulerRequest {
    /// A request of `kind`, every other field zero.
    pub const fn of(kind: u32) -> Self {
        Self {
            kind,
            status: Scheduler::RESPONSE_UNSET,
            task: 0,
            channel: 0,
            owner: 0,
            token: 0,
            value: 0,
            entry: core::ptr::null(),
            context: core::ptr::null_mut(),
        }
    }
}

/// `ChimiSelectSource` (chimi_fiber.h): one event of a select request; the
/// fields its `kind` names are set, the rest are zero.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SelectSource {
    pub kind: u32,
    pub channel: u64,
    pub owner: u64,
    pub token: u64,
    pub task: u64,
    pub deadline: i64,
}

/// `ChimiProcessRequest` (chimi_fiber.h): a process or supervisor request,
/// answered in place. Its `kind` and `status` share their offsets with
/// [`SchedulerRequest`]; the fields a kind names are set, the rest are zero.
#[repr(C)]
pub struct ProcessRequest {
    pub kind: u32,
    pub status: u32,
    pub address_kind: u32,
    pub restart: u32,
    pub address: u64,
    pub supervisor: u64,
    pub membership: u64,
    pub message: u64,
    pub name: u64,
    pub owner: u64,
    pub policy: *const c_void,
    pub spec: *const c_void,
    pub monitor: u64,
    pub queued: u64,
    pub capacity: u32,
    pub down: u32,
    pub carried: *const c_void,
}

impl ProcessRequest {
    /// A request of `kind`, every other field zero.
    pub const fn of(kind: u32) -> Self {
        Self {
            kind,
            status: Scheduler::RESPONSE_UNSET,
            address_kind: 0,
            restart: 0,
            address: 0,
            supervisor: 0,
            membership: 0,
            message: 0,
            name: 0,
            owner: 0,
            policy: core::ptr::null(),
            spec: core::ptr::null(),
            monitor: 0,
            queued: 0,
            capacity: 0,
            down: 0,
            carried: core::ptr::null(),
        }
    }
}

/// `ChimiMessageCapabilities` (chimi_fiber.h): the Receivers and Completions a
/// sent message carries, as a send request's context.
#[repr(C)]
pub struct MessageCapabilities {
    pub items: *const c_void,
    pub count: usize,
}

/// `ChimiTaskContext` (runtime.c): what a started task's fiber entry reads
/// when it first runs. `runtime.c`'s entry frees it.
#[repr(C)]
pub struct TaskContext {
    pub runtime: *mut Runtime,
    pub environment: *mut Environment,
    pub entry: *const c_void,
    pub captures: *mut RuntimeFragment,
    pub span: u64,
    pub parent: u64,
}

/// The scheduler's request kinds, replies and fiber commands (chimi_fiber.h)
/// the Rust intrinsics use.
pub struct Scheduler;

impl Scheduler {
    pub const FIBER_CANCEL: u32 = 2;
    pub const REQUEST_TASK_START: u32 = 1;
    pub const REQUEST_TASK_RESULT: u32 = 2;
    pub const REQUEST_TASK_CANCEL: u32 = 3;
    pub const REQUEST_CHANNEL_BOUNDED: u32 = 4;
    pub const REQUEST_CHANNEL_SEND: u32 = 5;
    pub const REQUEST_CHANNEL_RECEIVE: u32 = 6;
    pub const REQUEST_CHANNEL_CLOSE: u32 = 7;
    pub const REQUEST_CHANNEL_ABORT: u32 = 8;
    pub const REQUEST_CHANNEL_ADD_RECEIVER: u32 = 36;
    pub const REQUEST_CHANNEL_BROADCAST: u32 = 37;
    pub const REQUEST_CHANNEL_WATCH: u32 = 38;
    pub const REQUEST_CHANNEL_SUBSCRIBE: u32 = 39;
    pub const REQUEST_SCOPE_ENTER: u32 = 25;
    pub const REQUEST_SCOPE_EXIT: u32 = 26;
    pub const REQUEST_SCOPE_CHECK: u32 = 27;
    pub const REQUEST_RESOURCE_ENTER: u32 = 33;
    pub const REQUEST_RESOURCE_RELEASE: u32 = 34;
    pub const REQUEST_RESOURCE_EXIT: u32 = 35;
    pub const REQUEST_SCOPE_CANCEL: u32 = 41;
    pub const REQUEST_RESOURCE_USE: u32 = 49;
    pub const REQUEST_TASK_ANY: u32 = 42;
    pub const REQUEST_TIMER_NOW: u32 = 11;
    pub const REQUEST_TIMER_SLEEP_UNTIL: u32 = 12;
    pub const REQUEST_TASK_DRAIN: u32 = 43;
    pub const REQUEST_SELECT: u32 = 32;
    pub const SELECT_RECEIVE: u32 = 1;
    pub const SELECT_INBOX: u32 = 2;
    pub const SELECT_TASK: u32 = 3;
    pub const SELECT_TIMER: u32 = 4;
    pub const REQUEST_PROCESS_SELF: u32 = 13;
    pub const REQUEST_PROCESS_RECEIVE: u32 = 15;
    pub const REQUEST_SUPERVISOR_ADDRESS: u32 = 18;
    pub const REQUEST_SUPERVISOR_REMOVE_PROCESS: u32 = 19;
    pub const REQUEST_SUPERVISOR_STOP: u32 = 20;
    pub const REQUEST_SUPERVISOR_WAIT: u32 = 21;
    pub const REQUEST_SUPERVISOR_REMOVE_SUPERVISOR: u32 = 23;
    pub const REQUEST_PROCESS_UNMONITOR: u32 = 29;
    pub const REQUEST_PROCESS_MAILBOX: u32 = 30;
    pub const RESPONSE_FORCED: u32 = 16;
    pub const DOWN_RETURNED: u32 = 1;
    pub const DOWN_UNAVAILABLE: u32 = 5;
    pub const RESPONSE_UNSET: u32 = 0;
    pub const RESPONSE_OK: u32 = 1;
    pub const RESPONSE_PENDING: u32 = 2;
    pub const RESPONSE_SUCCEEDED: u32 = 3;
    pub const RESPONSE_CANCELLED: u32 = 4;
    pub const RESPONSE_DEFECTED: u32 = 5;
    pub const RESPONSE_STACK_OVERFLOW: u32 = 6;
    pub const RESPONSE_ERROR: u32 = 7;
    pub const RESPONSE_CLOSED: u32 = 8;
    pub const RESPONSE_ABORTED: u32 = 9;
    pub const RESPONSE_DRAINED: u32 = 10;
}

/// The codes of the program's core tree (runtime.h) the evaluator reads, and
/// the `HostFunction`s (runtime.c) a dependency method may name.
pub struct Core;

impl Core {
    pub const SYMBOL_FUNCTION: u32 = 0;
    pub const SYMBOL_CONSTANT: u32 = 1;
    pub const SYMBOL_REQUIREMENT: u32 = 2;
    pub const SYMBOL_LOCAL: u32 = 3;
    pub const SYMBOL_CONSTRUCTOR: u32 = 4;
    pub const SYMBOL_INTRINSIC: u32 = 5;
    pub const SYMBOL_PROCESS_NAME: u32 = 7;
    pub const LITERAL_INTEGER: u32 = 0;
    pub const LITERAL_TEXT: u32 = 1;
    pub const LITERAL_BOOLEAN: u32 = 2;
    pub const LITERAL_FLOAT: u32 = 3;
    pub const LITERAL_VOID: u32 = 4;
    pub const BINARY_ADD: u32 = 0;
    pub const BINARY_SUBTRACT: u32 = 1;
    pub const BINARY_MULTIPLY: u32 = 2;
    pub const BINARY_DIVIDE: u32 = 3;
    pub const BINARY_LESS: u32 = 4;
    pub const BINARY_LESS_EQUAL: u32 = 5;
    pub const BINARY_GREATER: u32 = 6;
    pub const BINARY_GREATER_EQUAL: u32 = 7;
    pub const BINARY_REMAINDER: u32 = 8;
    pub const BINARY_EQUAL: u32 = 9;
    pub const BINARY_RANGE: u32 = 10;
    pub const BINARY_RANGE_INCLUSIVE: u32 = 11;
    pub const PATTERN_WILDCARD: u32 = 0;
    pub const PATTERN_BINDING: u32 = 1;
    pub const PATTERN_LITERAL: u32 = 2;
    pub const PATTERN_CONSTRUCTOR: u32 = 3;
    pub const PATTERN_OUTCOME: u32 = 4;
    pub const PATTERN_LIST: u32 = 5;
    pub const PATTERN_ALIAS: u32 = 6;
    pub const PATTERN_TUPLE: u32 = 7;
    pub const TYPE_CONSOLE: u32 = 0;
    pub const TYPE_FILE_SYSTEM: u32 = 3;
    pub const TYPE_CLOCK: u32 = 37;
    pub const TYPE_RANDOM: u32 = 38;
    pub const HOST_PROCESS_UTF8_LINES: u32 = 0;
    pub const HOST_WRITE_LINE: u32 = 1;
    pub const HOST_WRITE_ERROR: u32 = 2;
    pub const HOST_RANDOM_NEXT_INT: u32 = 3;
    pub const HOST_CLOCK_NOW: u32 = 4;
    pub const HOST_CLOCK_TIMESTAMP: u32 = 5;
    pub const HOST_CLOCK_SLEEP_UNTIL: u32 = 6;
    pub const HOST_CLOCK_SHIFTED_TIMESTAMP: u32 = 7;
    pub const HOST_SELECT_SLEEP: u32 = 8;
}

/// The `CHIMI_INTRINSIC_*` ids (runtime.h) of the intrinsics Rust serves.
pub struct Intrinsic;

impl Intrinsic {
    pub const IO_PATH: u32 = 0;
    pub const LIST_LENGTH: u32 = 2;
    pub const TEXT_APPEND: u32 = 6;
    pub const TEXT_EQUAL: u32 = 7;
    pub const INT_PARSE: u32 = 16;
    pub const INT_TO_TEXT: u32 = 17;
    pub const INT_ADD_CHECKED: u32 = 18;
    pub const INT_SUBTRACT_CHECKED: u32 = 19;
    pub const INT_MULTIPLY_CHECKED: u32 = 20;
    pub const INT_DIVIDE_CHECKED: u32 = 21;
    pub const INT_REMAINDER_CHECKED: u32 = 22;
    pub const INT_MODULO: u32 = 23;
    pub const FLOAT_FROM_INT: u32 = 24;
    pub const FLOAT_TRUNCATE: u32 = 25;
    pub const FLOAT_FLOOR: u32 = 26;
    pub const FLOAT_CEILING: u32 = 27;
    pub const FLOAT_ROUND: u32 = 28;
    pub const FLOAT_TO_INT: u32 = 29;
    pub const FLOAT_TO_TEXT: u32 = 30;
    pub const FLOAT_PARSE: u32 = 31;
    pub const DEBUG_RENDER: u32 = 32;
    pub const DEBUG_RENDER_INDENTED: u32 = 33;
    pub const RANGE_START: u32 = 37;
    pub const RANGE_END: u32 = 38;
    pub const RANGE_LENGTH: u32 = 39;
    pub const RANGE_CONTAINS: u32 = 40;
    pub const PATH_FROM: u32 = 192;
    pub const PATH_TO_TEXT: u32 = 193;
    pub const MAP_EMPTY: u32 = 3;
    pub const MAP_GET: u32 = 4;
    pub const MAP_PUT: u32 = 5;
    pub const MAP_REMOVE: u32 = 133;
    pub const MAP_SIZE: u32 = 134;
    pub const MAP_REDUCE_ENTRIES: u32 = 135;
    pub const SET_EMPTY: u32 = 136;
    pub const SET_CONTAINS: u32 = 137;
    pub const SET_ADD: u32 = 138;
    pub const SET_REMOVE: u32 = 139;
    pub const SET_SIZE: u32 = 140;
    pub const SET_REDUCE_ITEMS: u32 = 141;
    pub const JSON_DECODE: u32 = 142;
    pub const JSON_ENCODE: u32 = 143;
    pub const JSON_INT: u32 = 144;
    pub const JSON_NUMBER_TO_INT: u32 = 145;
    pub const JSON_NUMBER_TO_DECIMAL: u32 = 146;
    pub const DECIMAL_SCALED: u32 = 147;
    pub const DECIMAL_READ: u32 = 148;
    pub const DECIMAL_TO_TEXT: u32 = 149;
    pub const DECIMAL_ADD_CHECKED: u32 = 150;
    pub const DECIMAL_SUBTRACT_CHECKED: u32 = 151;
    pub const DECIMAL_MULTIPLY_CHECKED: u32 = 152;
    pub const DECIMAL_DIVIDE_ROUNDED: u32 = 153;
    pub const DECIMAL_TO_FLOAT: u32 = 154;
    pub const BYTES_FROM: u32 = 155;
    pub const BYTES_LENGTH: u32 = 156;
    pub const BYTES_GET: u32 = 157;
    pub const BYTES_INDEX_OF: u32 = 158;
    pub const BYTES_CONCAT: u32 = 159;
    pub const BYTES_SLICE: u32 = 160;
    pub const BYTES_TO_LIST: u32 = 161;
    pub const BYTES_FROM_TEXT: u32 = 162;
    pub const BYTES_TO_TEXT_LOSSY: u32 = 163;
    pub const BYTES_VALID_UTF8_LENGTH: u32 = 164;
    pub const BYTES_READ_INT_CODE: u32 = 165;
    pub const BYTES_INT_BYTES: u32 = 166;
    pub const EMBED_BYTES: u32 = 168;
    pub const HTTP_COUNT_ENDS: u32 = 216;
    pub const DURATION_NANOSECONDS: u32 = 41;
    pub const DURATION_MICROSECONDS: u32 = 42;
    pub const DURATION_MILLISECONDS: u32 = 43;
    pub const DURATION_SECONDS: u32 = 44;
    pub const DURATION_MINUTES: u32 = 45;
    pub const DURATION_HOURS: u32 = 46;
    pub const DURATION_TO_NANOSECONDS: u32 = 47;
    pub const DURATION_TO_MICROSECONDS: u32 = 48;
    pub const DURATION_TO_MILLISECONDS: u32 = 49;
    pub const DURATION_TO_SECONDS: u32 = 50;
    pub const DURATION_ADD: u32 = 51;
    pub const DURATION_SUBTRACT: u32 = 52;
    pub const DURATION_MULTIPLY: u32 = 53;
    pub const DURATION_DIVIDE: u32 = 54;
    pub const DURATION_NEGATE: u32 = 55;
    pub const DURATION_ABSOLUTE: u32 = 56;
    pub const DURATION_TO_TEXT: u32 = 57;
    pub const DURATION_PARSE: u32 = 58;
    pub const DEBUG_CRASH: u32 = 114;
    pub const OPTION_UNWRAP_OR_FAIL: u32 = 115;
    pub const TEXT_CONCAT: u32 = 116;
    pub const TEXT_BYTE_LENGTH: u32 = 117;
    pub const TEXT_BYTE_SLICE: u32 = 118;
    pub const TEXT_CONTAINS: u32 = 119;
    pub const TEXT_STARTS_WITH: u32 = 120;
    pub const TEXT_ENDS_WITH: u32 = 121;
    pub const TEXT_SPLIT: u32 = 122;
    pub const TEXT_REPLACE: u32 = 123;
    pub const TEXT_TRIM_START: u32 = 124;
    pub const TEXT_TRIM_END: u32 = 125;
    pub const TEXT_LOWERCASE: u32 = 126;
    pub const TEXT_UPPERCASE: u32 = 127;
    pub const TEXT_GRAPHEMES: u32 = 128;
    pub const LIST_GET: u32 = 129;
    pub const LIST_PUSH: u32 = 130;
    pub const LIST_SLICE: u32 = 131;
    pub const LIST_SET: u32 = 132;
    pub const TASK_START: u32 = 8;
    pub const TASK_RESULT: u32 = 9;
    pub const TASK_CANCEL: u32 = 10;
    pub const CHANNEL_SEND: u32 = 11;
    pub const CHANNEL_RECEIVE: u32 = 12;
    pub const CHANNEL_CLOSE: u32 = 13;
    pub const CHANNEL_ABORT: u32 = 14;
    pub const CHANNEL_BOUNDED: u32 = 15;
    pub const CHANNEL_ADD_RECEIVER: u32 = 82;
    pub const BROADCAST_BOUNDED: u32 = 83;
    pub const BROADCAST_PUBLISH: u32 = 84;
    pub const BROADCAST_SUBSCRIBE: u32 = 85;
    pub const WATCH_STARTING_WITH: u32 = 86;
    pub const WATCH_PUBLISH: u32 = 87;
    pub const WATCH_SUBSCRIBE: u32 = 88;
    pub const WATCH_RECEIVE: u32 = 89;
    pub const CLOCK_SLEEP: u32 = 59;
    pub const CLOCK_NOW: u32 = 60;
    pub const CLOCK_TIMESTAMP: u32 = 61;
    pub const CLOCK_SINCE: u32 = 62;
    pub const CLOCK_SLEEP_UNTIL: u32 = 63;
    pub const CLOCK_ADD: u32 = 64;
    pub const CLOCK_BETWEEN: u32 = 65;
    pub const CLOCK_TIMESTAMP_TO_TEXT: u32 = 66;
    pub const CLOCK_UNIX_NANOSECONDS: u32 = 67;
    pub const CLOCK_FROM_UNIX_NANOSECONDS: u32 = 68;
    pub const CLOCK_TIMESTAMP_BETWEEN: u32 = 69;
    pub const CLOCK_STARTING_AT: u32 = 70;
    pub const TASK_TIMEOUT: u32 = 71;
    pub const TASK_DEADLINE: u32 = 72;
    pub const SELECT_RECEIVE: u32 = 76;
    pub const SELECT_INBOX: u32 = 77;
    pub const SELECT_TASK: u32 = 78;
    pub const SELECT_AFTER: u32 = 79;
    pub const SELECT_AT: u32 = 80;
    pub const SELECT_FIRST: u32 = 81;
    pub const SELECT_WATCH: u32 = 90;
    pub const PROCESS_SPEC: u32 = 91;
    pub const PROCESS_REGISTER: u32 = 92;
    pub const PROCESS_HANDOFF_SPEC: u32 = 100;
    pub const SUPERVISOR_PROCESS: u32 = 101;
    pub const SUPERVISOR_SUPERVISOR: u32 = 102;
    pub const SUPERVISOR_POLICY: u32 = 103;
    pub const SUPERVISOR_RESTARTS: u32 = 104;
    pub const PROCESS_RECEIVE: u32 = 93;
    pub const PROCESS_OWN_ADDRESS: u32 = 95;
    pub const PROCESS_QUEUED: u32 = 96;
    pub const PROCESS_CAPACITY: u32 = 97;
    pub const PROCESS_UNMONITOR: u32 = 99;
    pub const SUPERVISOR_ADDRESS: u32 = 109;
    pub const SUPERVISOR_REMOVE_PROCESS: u32 = 110;
    pub const SUPERVISOR_REMOVE_SUPERVISOR: u32 = 111;
    pub const SUPERVISOR_STOP: u32 = 112;
    pub const SUPERVISOR_WAIT: u32 = 113;
    pub const TASK_ALL: u32 = 73;
    pub const TASK_RESULTS: u32 = 74;
    pub const TASK_RACE: u32 = 75;
    pub const CAUSE_CONTEXT: u32 = 169;
    pub const CAUSE_OF: u32 = 170;
    pub const CAUSE_KIND_CODE: u32 = 171;
    pub const CAUSE_SUMMARY: u32 = 172;
    pub const CAUSE_FRAMES: u32 = 173;
    pub const CAUSE_CAUSED_BY: u32 = 174;
    pub const CAUSE_SUPPRESSED: u32 = 175;
    pub const CAUSE_REASON_CODE: u32 = 176;
    pub const CAUSE_TO_TEXT: u32 = 177;
    pub const RESOURCE_SCOPED: u32 = 182;
    pub const TEST_CAPTURE_DEFECT: u32 = 183;
    pub const TEST_ASSERT_EQUAL: u32 = 178;
    pub const TEST_ASSERT_NOT_EQUAL: u32 = 179;
    pub const TEST_ASSERT: u32 = 180;
    pub const TEST_FAIL: u32 = 181;
    pub const TEST_RECORDER: u32 = 184;
    pub const TEST_RECORD: u32 = 185;
    pub const TEST_RECORDED: u32 = 186;
    pub const TEST_SCRIPT: u32 = 187;
    pub const TEST_NEXT: u32 = 188;
    pub const TEST_MAILBOX: u32 = 189;
    pub const TEST_RECEIVED: u32 = 190;
    pub const ENV_HOST_VARIABLES: u32 = 194;
    pub const ENV_HOST_CURRENT_DIRECTORY: u32 = 195;
    pub const FS_HOST_OPEN: u32 = 196;
    pub const FS_HOST_CREATE: u32 = 197;
    pub const FS_HOST_READ: u32 = 198;
    pub const FS_HOST_WRITE: u32 = 199;
    pub const FS_HOST_CLOSE: u32 = 200;
    pub const FS_HOST_METADATA: u32 = 201;
    pub const FS_HOST_LIST: u32 = 202;
    pub const FS_HOST_CREATE_DIRECTORY: u32 = 203;
    pub const FS_HOST_REMOVE: u32 = 204;
    pub const FS_HOST_RENAME: u32 = 205;
    pub const FS_HOST_IS_DIRECTORY: u32 = 206;
    pub const FS_HOST_TEMPORARY_DIRECTORY: u32 = 207;
    pub const FS_TEST_ROOT: u32 = 208;
}

/// The address kind, beside `CHIMI_ADDRESS_NAMED` and `CHIMI_ADDRESS_DIRECT`
/// (chimi_fiber.h), whose target is a std/test Mailbox's id. No process owns
/// it, so it never reaches the scheduler.
pub const ADDRESS_TEST: u32 = 3;

/// The `CHIMI_CONSTRUCTOR_*` standard constructor ids (runtime.h) Rust builds.
pub struct StandardConstructor;

impl StandardConstructor {
    pub const OPTION_SOME: u32 = 6;
    pub const OPTION_NONE: u32 = 7;
    pub const DOWN_RETURNED: u32 = 44;
    pub const TIMEOUT_TIMED_OUT: u32 = 43;
    pub const RESULT_OK: u32 = 49;
    pub const RESULT_ERR: u32 = 50;
    pub const JSON_NULL: u32 = 8;
    pub const JSON_BOOLEAN: u32 = 9;
    pub const JSON_NUMBER: u32 = 10;
    pub const JSON_TEXT: u32 = 11;
    pub const JSON_ARRAY: u32 = 12;
    pub const JSON_OBJECT: u32 = 13;
    pub const DECODE_INVALID_JSON: u32 = 14;
    pub const JSON_NOT_INTEGER: u32 = 15;
    pub const JSON_OUT_OF_RANGE: u32 = 16;
    pub const INT_PARSE_EMPTY: u32 = 22;
    pub const INT_PARSE_INVALID_DIGIT: u32 = 23;
    pub const INT_PARSE_OUT_OF_RANGE: u32 = 24;
    pub const FLOAT_PARSE_EMPTY: u32 = 25;
    pub const FLOAT_PARSE_INVALID: u32 = 26;
    pub const FLOAT_PARSE_OUT_OF_RANGE: u32 = 27;
    pub const FLOAT_NOT_INTEGER: u32 = 28;
    pub const FLOAT_OUT_OF_RANGE: u32 = 29;
    pub const DURATION_PARSE_EMPTY: u32 = 33;
    pub const DURATION_PARSE_INVALID: u32 = 34;
    pub const PROCESS_SEND_FULL: u32 = 36;
    pub const DURATION_PARSE_OUT_OF_RANGE: u32 = 35;
    pub const CHANNEL_SEND_CLOSED: u32 = 17;
    pub const CHANNEL_SEND_ABORTED: u32 = 18;
    pub const CHANNEL_RECEIVED: u32 = 19;
    pub const CHANNEL_DRAINED: u32 = 20;
    pub const CHANNEL_ABORTED: u32 = 21;
}

/// `MapNode`.
#[repr(C)]
pub struct MapNode {
    pub key: *mut Value,
    pub value: *mut Value,
    pub left: *mut MapNode,
    pub right: *mut MapNode,
    pub height: u16,
}

/// `ChimiEnvironment`.
#[repr(C)]
pub struct Environment {
    pub locals: *mut *mut Value,
    pub requirements: *mut *mut Value,
    pub local_count: u32,
}

/// `ChimiCallable`.
#[repr(C)]
pub struct Callable {
    pub kind: CallableKind,
    pub symbol: u32,
    pub body: u32,
    pub parameters_offset: u32,
    pub parameters_length: u32,
    pub local_count: u32,
    pub environment: *mut Environment,
    pub inner: *mut Callable,
    pub requirement: u32,
    pub interface_symbol: u32,
    pub method: u32,
    pub name: *const c_char,
    pub intrinsic: u32,
    /// `HostFunction`.
    pub host: u32,
    pub shift: i64,
}

/// `ChimiControl`.
#[repr(C)]
pub struct Control {
    pub kind: ControlKind,
    pub value: *mut Value,
    pub message: *const c_char,
    pub node: i64,
    pub cause: *mut Value,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TextPayload {
    pub bytes: *const c_char,
    pub length: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ListPayload {
    pub vector: ListVector,
    pub start: u32,
    pub length: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RecordPayload {
    pub fields: *mut Field,
    pub length: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConstructedPayload {
    pub constructor: u32,
    pub type_name: *const c_char,
    pub variant_name: *const c_char,
    pub fields: *mut Field,
    pub length: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RangePayload {
    pub start: i64,
    pub end: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AddressPayload {
    pub kind: u32,
    pub target: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ManagedPayload {
    pub supervisor: u64,
    pub membership: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HandlePayload {
    pub slot: u32,
    pub generation: u32,
    pub name: *const c_char,
}

/// `ChimiValue`'s `as`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union Payload {
    pub integer: i64,
    pub floating: f64,
    pub text: TextPayload,
    pub boolean: bool,
    pub list: ListPayload,
    pub record: RecordPayload,
    pub constructed: ConstructedPayload,
    pub range: RangePayload,
    pub callable: *mut Callable,
    pub map: *mut MapNode,
    pub json_number: *const c_char,
    pub task: u64,
    pub capability: Capability,
    pub address: AddressPayload,
    pub managed: ManagedPayload,
    pub decimal: Decimal,
    pub handle: HandlePayload,
}

/// `ChimiValue`.
#[repr(C)]
pub struct Value {
    pub kind: ValueKind,
    pub payload: Payload,
}

/// `AllocationHeader`: precedes every heap allocation's payload.
#[repr(C)]
pub struct AllocationHeader {
    pub next: *mut AllocationHeader,
    pub area: *mut AllocationArea,
    pub bytes: usize,
    pub mark_epoch: u64,
    pub kind: AllocationKind,
    pub in_chunk: bool,
}

/// `YoungChunk`: its `data` follows, 16-byte aligned.
#[repr(C, align(16))]
pub struct YoungChunk {
    pub next: *mut YoungChunk,
    pub used: *mut u8,
    pub data: [u8; 0],
}

pub const YOUNG_CHUNK_BYTES: usize = 16 * 1024;
pub const YOUNG_CHUNK_CACHE: usize = 16;
pub const YOUNG_CHUNK_CAPACITY: usize = YOUNG_CHUNK_BYTES - offset_of!(YoungChunk, data);
pub const YOUNG_CHUNK_LARGEST: usize = YOUNG_CHUNK_CAPACITY / 4;

/// `AllocationArea`.
#[repr(C)]
pub struct AllocationArea {
    pub role: AreaRole,
    pub heap: *mut RuntimeHeap,
    pub scope: *mut AllocationScope,
    pub allocations: *mut AllocationHeader,
    pub bytes: usize,
    pub count: usize,
    pub next: *mut AllocationArea,
}

/// `AllocationScope`: one reclaiming region.
#[repr(C)]
pub struct AllocationScope {
    pub parent: *mut AllocationScope,
    pub heap: *mut RuntimeHeap,
    pub young: AllocationArea,
    pub adopted: *mut AllocationArea,
    pub survivors: AllocationArea,
    pub full_baseline: usize,
    pub has_baseline: bool,
    pub first_chunk: *mut YoungChunk,
    pub last_chunk: *mut YoungChunk,
    pub bump: *mut u8,
    pub bump_end: *mut u8,
    pub pending_bytes: usize,
    pub pending_count: usize,
    pub links_young: bool,
    pub live_bytes: usize,
    pub peak_live_bytes: usize,
    pub total_allocated_bytes: usize,
    pub allocated_bytes_by_kind: [usize; AllocationKind::COUNT],
    pub collections: u64,
    pub full_collections: u64,
    pub scanned_allocations: u64,
    pub promoted_bytes: usize,
}

/// `RuntimeHeap`.
#[repr(C)]
pub struct RuntimeHeap {
    pub owner: u64,
    pub profiled_node_plus_one: u32,
    pub live_bytes: usize,
    pub peak_live_bytes: usize,
    pub innermost: *mut AllocationScope,
    pub base: AllocationArea,
    pub adopted: *mut AllocationArea,
}

/// `HeapTotals`: one carrier's signed share of the process's accounting.
#[repr(C)]
pub struct HeapTotals {
    pub live_allocations: i64,
    pub footprint_bytes: i64,
    pub peak_footprint_bytes: i64,
    pub live_fragments: i64,
    pub fragment_footprint_bytes: i64,
    pub peak_fragment_footprint_bytes: i64,
    pub live_heaps: i64,
    pub cached_bytes: i64,
}

/// `ScopeTotals`.
#[repr(C)]
pub struct ScopeTotals {
    pub total_allocated_bytes: usize,
    pub allocated_bytes_by_kind: [usize; AllocationKind::COUNT],
    pub peak_live_bytes: usize,
    pub retained_bytes: usize,
    pub collections: u64,
    pub full_collections: u64,
    pub scanned_allocations: u64,
}

/// `FiberContext`: the heap the running fiber allocates in, and the area that
/// overrides it when set.
#[repr(C)]
pub struct FiberContext {
    pub heap: *mut RuntimeHeap,
    pub target: *mut AllocationArea,
}

/// `SmallBlock`: a freed block on a size class's free list.
#[repr(C)]
pub struct SmallBlock {
    pub next: *mut SmallBlock,
}

pub const SMALL_BLOCK_GRAIN: usize = 16;
pub const SMALL_BLOCK_LIMIT: usize = 256;
pub const SMALL_BLOCK_CACHE: usize = 4096;
/// The small-block size classes: one per grain up to the limit.
pub const SMALL_BLOCK_CLASSES: usize = SMALL_BLOCK_LIMIT / SMALL_BLOCK_GRAIN;

/// `LIST_BITS`: a list vector's trie is `1 << LIST_BITS` wide.
pub const LIST_BITS: u32 = 5;

/// `SmallBlockCache`: one carrier's small-block free lists, one per size
/// class, and their lengths (the cache is `small_blocks.rs`).
#[repr(C)]
pub struct SmallBlockCache {
    pub lists: [*mut SmallBlock; SMALL_BLOCK_CLASSES],
    pub counts: [u32; SMALL_BLOCK_CLASSES],
}

/// `PointerStack`: the growable stack marking and copying use instead of
/// recursion, since both run on 64 KiB fiber stacks.
#[repr(C)]
pub struct PointerStack {
    pub items: *mut *mut c_void,
    pub length: usize,
    pub capacity: usize,
}

/// `Carrier`: everything one carrier thread mutates while it runs fibers.
/// The allocator reads and writes the fields up to `small_blocks`; the
/// rest are mirrored so the record's size and every offset are checked.
#[repr(C)]
pub struct Carrier {
    pub fiber: FiberContext,
    pub heap_totals: HeapTotals,
    pub pending_footprint_bytes: i64,
    pub folded_footprint_bytes: i64,
    pub folded_fragment_footprint_bytes: i64,
    pub unscoped_allocated_bytes: u64,
    pub capture_transfer_probes: u64,
    pub process_heap_peak_value_bytes: usize,
    pub copy_counting: bool,
    pub copied_values: u64,
    pub copied_bytes: u64,
    pub fragment_copy_foreign_owner: u64,
    pub small_blocks: SmallBlockCache,
    pub young_chunk_cache: *mut YoungChunk,
    pub young_chunk_cache_count: u32,
    pub collecting_scope: *mut AllocationScope,
    pub collecting_full: bool,
    pub collecting_foreign: bool,
    pub collecting_epoch: u64,
    pub next_epoch: u64,
    pub end_epoch: u64,
    pub next_span: u64,
    pub end_span: u64,
    pub mark_stack: PointerStack,
    pub copy_stack: PointerStack,
    pub copy_pending: PointerStack,
    pub fs_read_chunk: *mut c_char,
    pub scope_totals: ScopeTotals,
    pub host_stack_limit: usize,
    /// `pthread_t`, which is an integer on Linux and a pointer on Apple
    /// platforms: one word on both, never read here, so it has no field
    /// fact of its own (the next field's offset pins it).
    pub thread: usize,
    pub bound: bool,
    pub console_lock_held: bool,
}

/// `ProfileRow`: one node's row of `--profile-file`, counted with relaxed
/// atomic adds.
#[repr(C)]
pub struct ProfileRow {
    pub operation: *const c_char,
    pub evaluations: u64,
    pub allocations: u64,
    pub allocated_bytes: u64,
}

pub const REGION_FULL_MIN_GROWTH: usize = 4 * 1024;
pub const MARK_EPOCH_BLOCK: usize = 4096;
pub const FRAGMENT_ROOTS: usize = 2;

/// `RuntimeFragment`: its area comes first, so an adopted fragment lives on
/// as that area.
#[repr(C)]
pub struct RuntimeFragment {
    pub area: AllocationArea,
    pub root_count: u32,
    pub roots: [*mut c_void; FRAGMENT_ROOTS],
}

/// What `runtime.c` must agree with, each written in the generated block as
/// `_Static_assert(<C expression> == <value>, ...)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fact {
    /// A C constant expression and its value: an enum constant, a struct's
    /// size or alignment, or the offset of a flexible array member, which
    /// has no size.
    Value { c: &'static str, value: usize },
    /// A field's offset and its size, so a field whose type changes inside
    /// its padding (a `uint16_t` widened to `uint32_t`) is caught too.
    /// `class` is the field's [`Class`], so a scalar's signedness and kind
    /// are checked as well.
    Field { ty: &'static str, field: &'static str, offset: usize, size: usize, class: Class },
}

/// What kind of scalar a field holds, which offsets and sizes cannot tell
/// apart (`int64_t` from `uint64_t`, `bool` from `uint8_t`, `double` from
/// `uint64_t`). The numbers are what `CHIMI_LAYOUT_CLASS` gives in C.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A pointer, struct, union or array: checked by offset and size only.
    Compound = 0,
    Signed = 1,
    Unsigned = 2,
    Bool = 3,
    Float = 4,
}

/// The [`Class`] of a mirrored field's type.
pub trait Classified {
    const CLASS: Class;
}

macro_rules! classified {
    ($class:ident: $($ty:ty),*) => { $(impl Classified for $ty { const CLASS: Class = Class::$class; })* };
}

classified!(Signed: i8, i16, i32, i64, isize);
classified!(Unsigned: u8, u16, u32, u64, usize, ValueKind, CallableKind, ControlKind, AllocationKind, AreaRole, HeapStatus);
classified!(Bool: bool);
classified!(Float: f32, f64);
classified!(Compound: Decimal, Capability, ListVector, TextPayload, ListPayload, RecordPayload,
    ConstructedPayload, RangePayload, AddressPayload, ManagedPayload, HandlePayload, Payload,
    AllocationArea, FiberContext, HeapTotals, ScopeTotals, SmallBlockCache, PointerStack,
    core::sync::atomic::AtomicU64);

impl<T> Classified for *const T {
    const CLASS: Class = Class::Compound;
}

impl<T> Classified for *mut T {
    const CLASS: Class = Class::Compound;
}

impl<T, const N: usize> Classified for [T; N] {
    const CLASS: Class = Class::Compound;
}

/// The size of the place `pointer` points at, so a field's size can be read
/// in a constant through `&raw const`.
const fn pointee_size<F>(_: *const F) -> usize {
    size_of::<F>()
}

/// The class of the place `pointer` points at.
const fn pointee_class<F: Classified>(_: *const F) -> Class {
    F::CLASS
}

macro_rules! facts {
    ($($kind:ident ($($arg:tt)*))*) => { &[$(facts!(@ $kind $($arg)*),)*] };
    (@ size $c:literal $rust:ty) => {
        Fact::Value { c: concat!("sizeof(", $c, ")"), value: size_of::<$rust>() }
    };
    (@ align $c:literal $rust:ty) => {
        Fact::Value { c: concat!("_Alignof(", $c, ")"), value: align_of::<$rust>() }
    };
    (@ field $c:literal $rust:ty, $cf:literal, $($rf:tt)+) => {
        Fact::Field {
            ty: $c,
            field: $cf,
            offset: offset_of!($rust, $($rf)+),
            size: {
                let place = core::mem::MaybeUninit::<$rust>::uninit();
                let pointer = place.as_ptr();
                // SAFETY: only the field's address is taken, never read, and
                // `pointer` points into the live `place`.
                pointee_size(unsafe { &raw const (*pointer).$($rf)+ })
            },
            class: {
                let place = core::mem::MaybeUninit::<$rust>::uninit();
                let pointer = place.as_ptr();
                // SAFETY: as for `size`.
                pointee_class(unsafe { &raw const (*pointer).$($rf)+ })
            },
        }
    };
    (@ constant $c:literal $value:expr) => {
        Fact::Value { c: $c, value: $value as usize }
    };
}

/// Every number the mirrors and `runtime.c` share.
pub const LAYOUT: &[Fact] = facts! {
    constant("LIST_BITS" LIST_BITS)
    constant("VALUE_VOID" ValueKind::VOID.0)
    constant("VALUE_INTEGER" ValueKind::INTEGER.0)
    constant("VALUE_FLOAT" ValueKind::FLOAT.0)
    constant("VALUE_TEXT" ValueKind::TEXT.0)
    constant("VALUE_BOOLEAN" ValueKind::BOOLEAN.0)
    constant("VALUE_LIST" ValueKind::LIST.0)
    constant("VALUE_RECORD" ValueKind::RECORD.0)
    constant("VALUE_CONSTRUCTED" ValueKind::CONSTRUCTED.0)
    constant("VALUE_CALLABLE" ValueKind::CALLABLE.0)
    constant("VALUE_MAP" ValueKind::MAP.0)
    constant("VALUE_JSON_NUMBER" ValueKind::JSON_NUMBER.0)
    constant("VALUE_TASK" ValueKind::TASK.0)
    constant("VALUE_CHANNEL_SENDER" ValueKind::CHANNEL_SENDER.0)
    constant("VALUE_CHANNEL_RECEIVER" ValueKind::CHANNEL_RECEIVER.0)
    constant("VALUE_CHANNEL_COMPLETION" ValueKind::CHANNEL_COMPLETION.0)
    constant("VALUE_TUPLE" ValueKind::TUPLE.0)
    constant("VALUE_RANGE" ValueKind::RANGE.0)
    constant("VALUE_DURATION" ValueKind::DURATION.0)
    constant("VALUE_INSTANT" ValueKind::INSTANT.0)
    constant("VALUE_TIMESTAMP" ValueKind::TIMESTAMP.0)
    constant("VALUE_DECIMAL" ValueKind::DECIMAL.0)
    constant("VALUE_PROCESS_ADDRESS" ValueKind::PROCESS_ADDRESS.0)
    constant("VALUE_PROCESS_INBOX" ValueKind::PROCESS_INBOX.0)
    constant("VALUE_SUPERVISOR_RUNNING" ValueKind::SUPERVISOR_RUNNING.0)
    constant("VALUE_MANAGED_PROCESS" ValueKind::MANAGED_PROCESS.0)
    constant("VALUE_MANAGED_SUPERVISOR" ValueKind::MANAGED_SUPERVISOR.0)
    constant("VALUE_PROCESS_MONITOR" ValueKind::PROCESS_MONITOR.0)
    constant("VALUE_SET" ValueKind::SET.0)
    constant("VALUE_BYTES" ValueKind::BYTES.0)
    constant("VALUE_CAUSE" ValueKind::CAUSE.0)
    constant("VALUE_TEST_RECORDER" ValueKind::TEST_RECORDER.0)
    constant("VALUE_TEST_SCRIPT" ValueKind::TEST_SCRIPT.0)
    constant("VALUE_TEST_MAILBOX" ValueKind::TEST_MAILBOX.0)
    constant("VALUE_NATIVE_HANDLE" ValueKind::NATIVE_HANDLE.0)
    constant("sizeof(ValueKind)" size_of::<ValueKind>())
    constant("CALL_DECLARED" CallableKind::DECLARED.0)
    constant("CALL_CLOSURE" CallableKind::CLOSURE.0)
    constant("CALL_DEPENDENCY" CallableKind::DEPENDENCY.0)
    constant("CALL_HOST" CallableKind::HOST.0)
    constant("CALL_INTRINSIC" CallableKind::INTRINSIC.0)
    constant("CALL_CONSTRUCTOR" CallableKind::CONSTRUCTOR.0)
    constant("sizeof(CallableKind)" size_of::<CallableKind>())
    constant("sizeof(HostFunction)" size_of::<u32>())
    constant("CONTROL_SUCCESS" ControlKind::SUCCESS.0)
    constant("CONTROL_FAILURE" ControlKind::FAILURE.0)
    constant("CONTROL_DEFECT" ControlKind::DEFECT.0)
    constant("CONTROL_CANCEL" ControlKind::CANCEL.0)
    constant("CONTROL_BREAK" ControlKind::BREAK.0)
    constant("sizeof(ControlKind)" size_of::<ControlKind>())
    constant("ALLOCATION_BYTES" AllocationKind::BYTES.0)
    constant("ALLOCATION_VALUE" AllocationKind::VALUE.0)
    constant("ALLOCATION_CONTROL" AllocationKind::CONTROL.0)
    constant("ALLOCATION_ENVIRONMENT" AllocationKind::ENVIRONMENT.0)
    constant("ALLOCATION_VALUE_POINTERS" AllocationKind::VALUE_POINTERS.0)
    constant("ALLOCATION_FIELDS" AllocationKind::FIELDS.0)
    constant("ALLOCATION_CALLABLE" AllocationKind::CALLABLE.0)
    constant("ALLOCATION_MAP_NODE" AllocationKind::MAP_NODE.0)
    constant("ALLOCATION_LIST_NODE" AllocationKind::LIST_NODE.0)
    constant("ALLOCATION_FRAME_ENVIRONMENT" AllocationKind::FRAME_ENVIRONMENT.0)
    constant("ALLOCATION_KIND_COUNT" AllocationKind::COUNT)
    constant("sizeof(AllocationKind)" size_of::<AllocationKind>())
    constant("AREA_YOUNG" AreaRole::YOUNG.0)
    constant("AREA_SURVIVOR" AreaRole::SURVIVOR.0)
    constant("AREA_BASE" AreaRole::BASE.0)
    constant("AREA_FRAGMENT" AreaRole::FRAGMENT.0)
    constant("AREA_STATIC" AreaRole::STATIC.0)
    constant("sizeof(AreaRole)" size_of::<AreaRole>())
    constant("YOUNG_CHUNK_BYTES" YOUNG_CHUNK_BYTES)
    constant("YOUNG_CHUNK_CACHE" YOUNG_CHUNK_CACHE)
    constant("YOUNG_CHUNK_CAPACITY" YOUNG_CHUNK_CAPACITY)
    constant("YOUNG_CHUNK_LARGEST" YOUNG_CHUNK_LARGEST)
    constant("SMALL_BLOCK_GRAIN" SMALL_BLOCK_GRAIN)
    constant("SMALL_BLOCK_LIMIT" SMALL_BLOCK_LIMIT)
    constant("SMALL_BLOCK_CACHE" SMALL_BLOCK_CACHE)
    constant("FRAGMENT_ROOTS" FRAGMENT_ROOTS)
    constant("REGION_FULL_MIN_GROWTH" REGION_FULL_MIN_GROWTH)
    constant("MARK_EPOCH_BLOCK" MARK_EPOCH_BLOCK)
    constant("HEAP_OK" HeapStatus::OK.0)
    constant("HEAP_FOREIGN_POINTER" HeapStatus::FOREIGN_POINTER.0)
    constant("sizeof(HeapStatus)" size_of::<HeapStatus>())

    size("ChimiDecimal" Decimal)
    align("ChimiDecimal" Decimal)
    field("ChimiDecimal" Decimal, "high", high)
    field("ChimiDecimal" Decimal, "low", low)
    field("ChimiDecimal" Decimal, "scale", scale)
    field("ChimiDecimal" Decimal, "negative", negative)
    size("RuntimeField" Field)
    field("RuntimeField" Field, "name", name)
    field("RuntimeField" Field, "value", value)
    size("RuntimeCapability" Capability)
    field("RuntimeCapability" Capability, "channel", channel)
    field("RuntimeCapability" Capability, "owner", owner)
    field("RuntimeCapability" Capability, "token", token)
    size("ListNode" ListNode)
    align("ListNode" ListNode)
    field("ListNode" ListNode, "height", height)
    field("ListNode" ListNode, "count", count)
    constant("offsetof(ListNode, slots)" offset_of!(ListNode, slots))
    size("ListVector" ListVector)
    field("ListVector" ListVector, "root", root)
    field("ListVector" ListVector, "tail", tail)
    field("ListVector" ListVector, "length", length)
    size("ChimiOrigin" Origin)
    field("ChimiOrigin" Origin, "file_id", file_id)
    field("ChimiOrigin" Origin, "semantic_id", semantic_id)
    field("ChimiOrigin" Origin, "start", start)
    field("ChimiOrigin" Origin, "end", end)
    field("ChimiOrigin" Origin, "location", location)
    size("ChimiNode" Node)
    field("ChimiNode" Node, "origin", origin)
    field("ChimiNode" Node, "a", a)
    field("ChimiNode" Node, "b", b)
    field("ChimiNode" Node, "c", c)
    field("ChimiNode" Node, "d", d)
    field("ChimiNode" Node, "offset", offset)
    field("ChimiNode" Node, "length", length)
    field("ChimiNode" Node, "integer", integer)
    size("ChimiInjectedSignal" InjectedSignal)
    field("ChimiInjectedSignal" InjectedSignal, "code", code)
    field("ChimiInjectedSignal" InjectedSignal, "at", at)
    size("ChimiRuntime" Runtime)
    field("ChimiRuntime" Runtime, "execution_id", execution_id)
    field("ChimiRuntime" Runtime, "computation", computation)
    field("ChimiRuntime" Runtime, "full_trace", full_trace)
    field("ChimiRuntime" Runtime, "virtual_clock", virtual_clock)
    field("ChimiRuntime" Runtime, "carriers", carriers)
    field("ChimiRuntime" Runtime, "clock_start", clock_start)
    field("ChimiRuntime" Runtime, "start_unix_ns", start_unix_ns)
    field("ChimiRuntime" Runtime, "deadlock_waiters", deadlock_waiters)
    field("ChimiRuntime" Runtime, "last_defect_incarnation", last_defect_incarnation)
    field("ChimiRuntime" Runtime, "last_defect_membership", last_defect_membership)
    field("ChimiRuntime" Runtime, "last_defect_message", last_defect_message)
    field("ChimiRuntime" Runtime, "process_defect_count", process_defect_count)
    field("ChimiRuntime" Runtime, "capability_violations", capability_violations)
    field("ChimiRuntime" Runtime, "cancel", cancel)
    field("ChimiRuntime" Runtime, "injected_signal", injected_signal)
    field("ChimiRuntime" Runtime, "injected_signal_at", injected_signal_at)
    field("ChimiRuntime" Runtime, "injected_user_signals", injected_user_signals)
    field("ChimiRuntime" Runtime, "injected_user_signal_count", injected_user_signal_count)
    field("ChimiRuntime" Runtime, "silent", silent)
    field("ChimiRuntime" Runtime, "telemetry", telemetry)
    field("ChimiRuntime" Runtime, "outcome", outcome)
    field("ChimiRuntime" Runtime, "max_line_bytes", max_line_bytes)
    field("ChimiRuntime" Runtime, "log_level", log_level)
    field("ChimiRuntime" Runtime, "log_file", log_file)
    field("ChimiRuntime" Runtime, "metrics_file", metrics_file)
    field("ChimiRuntime" Runtime, "next_sequence", next_sequence)
    field("ChimiRuntime" Runtime, "next_span", next_span)
    field("ChimiRuntime" Runtime, "trace_events_constructed", trace_events_constructed)
    size("ChimiSchedulerTraceEvent" SchedulerTraceEvent)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "family", family)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "kind", kind)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "sequence", sequence)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "task", task)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "parent", parent)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "cause", cause)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "channel", channel)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "capacity", capacity)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "queue_depth", queue_depth)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "waiter_depth", waiter_depth)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "discard_count", discard_count)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "supervisor", supervisor)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "membership", membership)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "incarnation", incarnation)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "name", name)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "parent_supervisor", parent_supervisor)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "attempts_in_window", attempts_in_window)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "time", time)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "address_kind", address_kind)
    field("ChimiSchedulerTraceEvent" SchedulerTraceEvent, "outcome", outcome)
    constant("CHIMI_MAX_INJECTED_SIGNALS" MAX_INJECTED_SIGNALS)
    constant("CHIMI_SCHEDULER_TRACE_TASK" Trace::FAMILY_TASK)
    constant("CHIMI_SCHEDULER_TRACE_CHANNEL" Trace::FAMILY_CHANNEL)
    constant("CHIMI_SCHEDULER_TRACE_PROCESS" Trace::FAMILY_PROCESS)
    constant("CHIMI_SCHEDULER_TRACE_CLOCK" Trace::FAMILY_CLOCK)
    constant("CHIMI_PROCESS_TRACE_ADMISSION_REJECTED" Trace::PROCESS_ADMISSION_REJECTED)
    constant("CHIMI_PROCESS_TRACE_TERMINATED" Trace::PROCESS_TERMINATED)
    constant("CHIMI_PROCESS_TRACE_SEND" Trace::PROCESS_SEND)
    constant("CHIMI_PROCESS_CAUSE_DEFECTED" Trace::PROCESS_CAUSE_DEFECTED)
    constant("CHIMI_PROCESS_DEFECT_STACK_OVERFLOW" Trace::PROCESS_DEFECT_STACK_OVERFLOW)
    constant("CHIMI_CLOCK_TRACE_SELECT_COMMITTED" Trace::CLOCK_SELECT_COMMITTED)
    constant("CHIMI_ADDRESS_NAMED" Trace::ADDRESS_NAMED)
    constant("CHIMI_ADDRESS_DIRECT" Trace::ADDRESS_DIRECT)
    size("ChimiBindingCall" BindingCall)
    field("ChimiBindingCall" BindingCall, "runtime", runtime)
    field("ChimiBindingCall" BindingCall, "arguments", arguments)
    field("ChimiBindingCall" BindingCall, "argument_count", argument_count)
    field("ChimiBindingCall" BindingCall, "call_node", call_node)
    field("ChimiBindingCall" BindingCall, "held", held)
    size("ChimiSymbol" Symbol)
    field("ChimiSymbol" Symbol, "kind", kind)
    field("ChimiSymbol" Symbol, "name", name)
    field("ChimiSymbol" Symbol, "function", function)
    field("ChimiSymbol" Symbol, "constant", constant)
    field("ChimiSymbol" Symbol, "constructor", constructor)
    field("ChimiSymbol" Symbol, "intrinsic", intrinsic)
    field("ChimiSymbol" Symbol, "standard_constructor", standard_constructor)
    field("ChimiSymbol" Symbol, "standard_type", standard_type)
    field("ChimiSymbol" Symbol, "environment_slot", environment_slot)
    field("ChimiSymbol" Symbol, "binding", binding)
    size("ChimiFunction" Function)
    field("ChimiFunction" Function, "symbol", symbol)
    field("ChimiFunction" Function, "body", body)
    field("ChimiFunction" Function, "parameters_offset", parameters_offset)
    field("ChimiFunction" Function, "parameters_length", parameters_length)
    field("ChimiFunction" Function, "local_count", local_count)
    size("ChimiNamedNode" NamedNode)
    field("ChimiNamedNode" NamedNode, "name", name)
    field("ChimiNamedNode" NamedNode, "node", node)
    size("ChimiStep" Step)
    field("ChimiStep" Step, "kind", kind)
    field("ChimiStep" Step, "symbol", symbol)
    field("ChimiStep" Step, "node", node)
    size("ChimiArm" Arm)
    field("ChimiArm" Arm, "patterns_offset", patterns_offset)
    field("ChimiArm" Arm, "patterns_length", patterns_length)
    field("ChimiArm" Arm, "guard", guard)
    field("ChimiArm" Arm, "body", body)
    field("ChimiArm" Arm, "cause_subject", cause_subject)
    field("ChimiArm" Arm, "cause_local", cause_local)
    size("ChimiSchedulerRequest" SchedulerRequest)
    field("ChimiSchedulerRequest" SchedulerRequest, "kind", kind)
    field("ChimiSchedulerRequest" SchedulerRequest, "status", status)
    field("ChimiSchedulerRequest" SchedulerRequest, "task", task)
    field("ChimiSchedulerRequest" SchedulerRequest, "channel", channel)
    field("ChimiSchedulerRequest" SchedulerRequest, "owner", owner)
    field("ChimiSchedulerRequest" SchedulerRequest, "token", token)
    field("ChimiSchedulerRequest" SchedulerRequest, "value", value)
    field("ChimiSchedulerRequest" SchedulerRequest, "entry", entry)
    field("ChimiSchedulerRequest" SchedulerRequest, "context", context)
    size("ChimiMessageCapabilities" MessageCapabilities)
    field("ChimiMessageCapabilities" MessageCapabilities, "items", items)
    field("ChimiMessageCapabilities" MessageCapabilities, "count", count)
    constant("CHIMI_FIBER_CANCEL" Scheduler::FIBER_CANCEL)
    constant("CHIMI_REQUEST_TASK_START" Scheduler::REQUEST_TASK_START)
    constant("CHIMI_REQUEST_TASK_RESULT" Scheduler::REQUEST_TASK_RESULT)
    constant("CHIMI_REQUEST_TASK_ANY" Scheduler::REQUEST_TASK_ANY)
    constant("CHIMI_REQUEST_TIMER_NOW" Scheduler::REQUEST_TIMER_NOW)
    constant("CHIMI_REQUEST_TIMER_SLEEP_UNTIL" Scheduler::REQUEST_TIMER_SLEEP_UNTIL)
    constant("CHIMI_REQUEST_TASK_DRAIN" Scheduler::REQUEST_TASK_DRAIN)
    constant("CHIMI_REQUEST_SELECT" Scheduler::REQUEST_SELECT)
    constant("CHIMI_SELECT_RECEIVE" Scheduler::SELECT_RECEIVE)
    constant("CHIMI_SELECT_INBOX" Scheduler::SELECT_INBOX)
    constant("CHIMI_SELECT_TASK" Scheduler::SELECT_TASK)
    constant("CHIMI_SELECT_TIMER" Scheduler::SELECT_TIMER)
    constant("CHIMI_REQUEST_PROCESS_SELF" Scheduler::REQUEST_PROCESS_SELF)
    constant("CHIMI_REQUEST_PROCESS_RECEIVE" Scheduler::REQUEST_PROCESS_RECEIVE)
    constant("CHIMI_REQUEST_SUPERVISOR_ADDRESS" Scheduler::REQUEST_SUPERVISOR_ADDRESS)
    constant("CHIMI_REQUEST_SUPERVISOR_REMOVE_PROCESS" Scheduler::REQUEST_SUPERVISOR_REMOVE_PROCESS)
    constant("CHIMI_REQUEST_SUPERVISOR_STOP" Scheduler::REQUEST_SUPERVISOR_STOP)
    constant("CHIMI_REQUEST_SUPERVISOR_WAIT" Scheduler::REQUEST_SUPERVISOR_WAIT)
    constant("CHIMI_REQUEST_SUPERVISOR_REMOVE_SUPERVISOR" Scheduler::REQUEST_SUPERVISOR_REMOVE_SUPERVISOR)
    constant("CHIMI_REQUEST_PROCESS_UNMONITOR" Scheduler::REQUEST_PROCESS_UNMONITOR)
    constant("CHIMI_REQUEST_PROCESS_MAILBOX" Scheduler::REQUEST_PROCESS_MAILBOX)
    constant("CHIMI_RESPONSE_FORCED" Scheduler::RESPONSE_FORCED)
    constant("CHIMI_DOWN_RETURNED" Scheduler::DOWN_RETURNED)
    constant("CHIMI_DOWN_UNAVAILABLE" Scheduler::DOWN_UNAVAILABLE)
    size("ChimiProcessRequest" ProcessRequest)
    field("ChimiProcessRequest" ProcessRequest, "kind", kind)
    field("ChimiProcessRequest" ProcessRequest, "status", status)
    field("ChimiProcessRequest" ProcessRequest, "address_kind", address_kind)
    field("ChimiProcessRequest" ProcessRequest, "restart", restart)
    field("ChimiProcessRequest" ProcessRequest, "address", address)
    field("ChimiProcessRequest" ProcessRequest, "supervisor", supervisor)
    field("ChimiProcessRequest" ProcessRequest, "membership", membership)
    field("ChimiProcessRequest" ProcessRequest, "message", message)
    field("ChimiProcessRequest" ProcessRequest, "name", name)
    field("ChimiProcessRequest" ProcessRequest, "owner", owner)
    field("ChimiProcessRequest" ProcessRequest, "policy", policy)
    field("ChimiProcessRequest" ProcessRequest, "spec", spec)
    field("ChimiProcessRequest" ProcessRequest, "monitor", monitor)
    field("ChimiProcessRequest" ProcessRequest, "queued", queued)
    field("ChimiProcessRequest" ProcessRequest, "capacity", capacity)
    field("ChimiProcessRequest" ProcessRequest, "down", down)
    field("ChimiProcessRequest" ProcessRequest, "carried", carried)
    size("ChimiSelectSource" SelectSource)
    field("ChimiSelectSource" SelectSource, "kind", kind)
    field("ChimiSelectSource" SelectSource, "channel", channel)
    field("ChimiSelectSource" SelectSource, "owner", owner)
    field("ChimiSelectSource" SelectSource, "token", token)
    field("ChimiSelectSource" SelectSource, "task", task)
    field("ChimiSelectSource" SelectSource, "deadline", deadline)
    constant("CHIMI_RESPONSE_ERROR" Scheduler::RESPONSE_ERROR)
    size("ChimiTaskContext" TaskContext)
    field("ChimiTaskContext" TaskContext, "runtime", runtime)
    field("ChimiTaskContext" TaskContext, "environment", environment)
    field("ChimiTaskContext" TaskContext, "entry", entry)
    field("ChimiTaskContext" TaskContext, "captures", captures)
    field("ChimiTaskContext" TaskContext, "span", span)
    field("ChimiTaskContext" TaskContext, "parent", parent)
    constant("CHIMI_REQUEST_TASK_CANCEL" Scheduler::REQUEST_TASK_CANCEL)
    constant("CHIMI_REQUEST_CHANNEL_BOUNDED" Scheduler::REQUEST_CHANNEL_BOUNDED)
    constant("CHIMI_REQUEST_CHANNEL_SEND" Scheduler::REQUEST_CHANNEL_SEND)
    constant("CHIMI_REQUEST_CHANNEL_RECEIVE" Scheduler::REQUEST_CHANNEL_RECEIVE)
    constant("CHIMI_REQUEST_CHANNEL_CLOSE" Scheduler::REQUEST_CHANNEL_CLOSE)
    constant("CHIMI_REQUEST_CHANNEL_ABORT" Scheduler::REQUEST_CHANNEL_ABORT)
    constant("CHIMI_REQUEST_CHANNEL_ADD_RECEIVER" Scheduler::REQUEST_CHANNEL_ADD_RECEIVER)
    constant("CHIMI_REQUEST_CHANNEL_BROADCAST" Scheduler::REQUEST_CHANNEL_BROADCAST)
    constant("CHIMI_REQUEST_CHANNEL_WATCH" Scheduler::REQUEST_CHANNEL_WATCH)
    constant("CHIMI_REQUEST_CHANNEL_SUBSCRIBE" Scheduler::REQUEST_CHANNEL_SUBSCRIBE)
    constant("CHIMI_RESPONSE_SUCCEEDED" Scheduler::RESPONSE_SUCCEEDED)
    constant("CHIMI_RESPONSE_CANCELLED" Scheduler::RESPONSE_CANCELLED)
    constant("CHIMI_RESPONSE_DEFECTED" Scheduler::RESPONSE_DEFECTED)
    constant("CHIMI_RESPONSE_STACK_OVERFLOW" Scheduler::RESPONSE_STACK_OVERFLOW)
    constant("CHIMI_RESPONSE_CLOSED" Scheduler::RESPONSE_CLOSED)
    constant("CHIMI_RESPONSE_ABORTED" Scheduler::RESPONSE_ABORTED)
    constant("CHIMI_RESPONSE_DRAINED" Scheduler::RESPONSE_DRAINED)
    constant("CHIMI_REQUEST_SCOPE_ENTER" Scheduler::REQUEST_SCOPE_ENTER)
    constant("CHIMI_REQUEST_SCOPE_EXIT" Scheduler::REQUEST_SCOPE_EXIT)
    constant("CHIMI_REQUEST_SCOPE_CHECK" Scheduler::REQUEST_SCOPE_CHECK)
    constant("CHIMI_REQUEST_RESOURCE_ENTER" Scheduler::REQUEST_RESOURCE_ENTER)
    constant("CHIMI_REQUEST_RESOURCE_RELEASE" Scheduler::REQUEST_RESOURCE_RELEASE)
    constant("CHIMI_REQUEST_RESOURCE_EXIT" Scheduler::REQUEST_RESOURCE_EXIT)
    constant("CHIMI_REQUEST_SCOPE_CANCEL" Scheduler::REQUEST_SCOPE_CANCEL)
    constant("CHIMI_REQUEST_RESOURCE_USE" Scheduler::REQUEST_RESOURCE_USE)
    constant("CHIMI_RESPONSE_UNSET" Scheduler::RESPONSE_UNSET)
    constant("CHIMI_RESPONSE_OK" Scheduler::RESPONSE_OK)
    constant("CHIMI_RESPONSE_PENDING" Scheduler::RESPONSE_PENDING)
    size("ChimiPattern" Pattern)
    field("ChimiPattern" Pattern, "kind", kind)
    field("ChimiPattern" Pattern, "a", a)
    field("ChimiPattern" Pattern, "offset", offset)
    field("ChimiPattern" Pattern, "length", length)
    field("ChimiPattern" Pattern, "integer", integer)
    constant("CHIMI_SYMBOL_FUNCTION" Core::SYMBOL_FUNCTION)
    constant("CHIMI_SYMBOL_CONSTANT" Core::SYMBOL_CONSTANT)
    constant("CHIMI_SYMBOL_REQUIREMENT" Core::SYMBOL_REQUIREMENT)
    constant("CHIMI_SYMBOL_LOCAL" Core::SYMBOL_LOCAL)
    constant("CHIMI_SYMBOL_CONSTRUCTOR" Core::SYMBOL_CONSTRUCTOR)
    constant("CHIMI_SYMBOL_INTRINSIC" Core::SYMBOL_INTRINSIC)
    constant("CHIMI_SYMBOL_PROCESS_NAME" Core::SYMBOL_PROCESS_NAME)
    constant("CHIMI_LITERAL_INTEGER" Core::LITERAL_INTEGER)
    constant("CHIMI_LITERAL_TEXT" Core::LITERAL_TEXT)
    constant("CHIMI_LITERAL_BOOLEAN" Core::LITERAL_BOOLEAN)
    constant("CHIMI_LITERAL_FLOAT" Core::LITERAL_FLOAT)
    constant("CHIMI_LITERAL_VOID" Core::LITERAL_VOID)
    constant("CHIMI_BINARY_ADD" Core::BINARY_ADD)
    constant("CHIMI_BINARY_SUBTRACT" Core::BINARY_SUBTRACT)
    constant("CHIMI_BINARY_MULTIPLY" Core::BINARY_MULTIPLY)
    constant("CHIMI_BINARY_DIVIDE" Core::BINARY_DIVIDE)
    constant("CHIMI_BINARY_LESS" Core::BINARY_LESS)
    constant("CHIMI_BINARY_LESS_EQUAL" Core::BINARY_LESS_EQUAL)
    constant("CHIMI_BINARY_GREATER" Core::BINARY_GREATER)
    constant("CHIMI_BINARY_GREATER_EQUAL" Core::BINARY_GREATER_EQUAL)
    constant("CHIMI_BINARY_REMAINDER" Core::BINARY_REMAINDER)
    constant("CHIMI_BINARY_EQUAL" Core::BINARY_EQUAL)
    constant("CHIMI_BINARY_RANGE" Core::BINARY_RANGE)
    constant("CHIMI_BINARY_RANGE_INCLUSIVE" Core::BINARY_RANGE_INCLUSIVE)
    constant("CHIMI_PATTERN_WILDCARD" Core::PATTERN_WILDCARD)
    constant("CHIMI_PATTERN_BINDING" Core::PATTERN_BINDING)
    constant("CHIMI_PATTERN_LITERAL" Core::PATTERN_LITERAL)
    constant("CHIMI_PATTERN_CONSTRUCTOR" Core::PATTERN_CONSTRUCTOR)
    constant("CHIMI_PATTERN_OUTCOME" Core::PATTERN_OUTCOME)
    constant("CHIMI_PATTERN_LIST" Core::PATTERN_LIST)
    constant("CHIMI_PATTERN_ALIAS" Core::PATTERN_ALIAS)
    constant("CHIMI_PATTERN_TUPLE" Core::PATTERN_TUPLE)
    constant("CHIMI_TYPE_CONSOLE" Core::TYPE_CONSOLE)
    constant("CHIMI_TYPE_FILE_SYSTEM" Core::TYPE_FILE_SYSTEM)
    constant("CHIMI_TYPE_CLOCK" Core::TYPE_CLOCK)
    constant("CHIMI_TYPE_RANDOM" Core::TYPE_RANDOM)
    constant("HOST_PROCESS_UTF8_LINES" Core::HOST_PROCESS_UTF8_LINES)
    constant("HOST_WRITE_LINE" Core::HOST_WRITE_LINE)
    constant("HOST_WRITE_ERROR" Core::HOST_WRITE_ERROR)
    constant("HOST_RANDOM_NEXT_INT" Core::HOST_RANDOM_NEXT_INT)
    constant("HOST_CLOCK_NOW" Core::HOST_CLOCK_NOW)
    constant("HOST_CLOCK_TIMESTAMP" Core::HOST_CLOCK_TIMESTAMP)
    constant("HOST_CLOCK_SLEEP_UNTIL" Core::HOST_CLOCK_SLEEP_UNTIL)
    constant("HOST_CLOCK_SHIFTED_TIMESTAMP" Core::HOST_CLOCK_SHIFTED_TIMESTAMP)
    constant("HOST_SELECT_SLEEP" Core::HOST_SELECT_SLEEP)
    size("ChimiConstructor" Constructor)
    field("ChimiConstructor" Constructor, "type_name", type_name)
    field("ChimiConstructor" Constructor, "variant_name", variant_name)
    field("ChimiConstructor" Constructor, "fields_offset", fields_offset)
    field("ChimiConstructor" Constructor, "fields_length", fields_length)
    field("ChimiConstructor" Constructor, "outcome_success", outcome_success)
    field("ChimiConstructor" Constructor, "ordinal", ordinal)
    field("ChimiConstructor" Constructor, "program_exit", program_exit)
    constant("CHIMI_INTRINSIC_MAP_EMPTY" Intrinsic::MAP_EMPTY)
    constant("CHIMI_INTRINSIC_MAP_GET" Intrinsic::MAP_GET)
    constant("CHIMI_INTRINSIC_MAP_PUT" Intrinsic::MAP_PUT)
    constant("CHIMI_INTRINSIC_MAP_REMOVE" Intrinsic::MAP_REMOVE)
    constant("CHIMI_INTRINSIC_MAP_SIZE" Intrinsic::MAP_SIZE)
    constant("CHIMI_INTRINSIC_MAP_REDUCE_ENTRIES" Intrinsic::MAP_REDUCE_ENTRIES)
    constant("CHIMI_INTRINSIC_SET_EMPTY" Intrinsic::SET_EMPTY)
    constant("CHIMI_INTRINSIC_SET_CONTAINS" Intrinsic::SET_CONTAINS)
    constant("CHIMI_INTRINSIC_SET_ADD" Intrinsic::SET_ADD)
    constant("CHIMI_INTRINSIC_SET_REMOVE" Intrinsic::SET_REMOVE)
    constant("CHIMI_INTRINSIC_SET_SIZE" Intrinsic::SET_SIZE)
    constant("CHIMI_INTRINSIC_SET_REDUCE_ITEMS" Intrinsic::SET_REDUCE_ITEMS)
    constant("CHIMI_CONSTRUCTOR_OPTION_SOME" StandardConstructor::OPTION_SOME)
    constant("CHIMI_CONSTRUCTOR_CHANNEL_SEND_CLOSED" StandardConstructor::CHANNEL_SEND_CLOSED)
    constant("CHIMI_CONSTRUCTOR_CHANNEL_SEND_ABORTED" StandardConstructor::CHANNEL_SEND_ABORTED)
    constant("CHIMI_CONSTRUCTOR_CHANNEL_RECEIVED" StandardConstructor::CHANNEL_RECEIVED)
    constant("CHIMI_CONSTRUCTOR_CHANNEL_DRAINED" StandardConstructor::CHANNEL_DRAINED)
    constant("CHIMI_CONSTRUCTOR_CHANNEL_ABORTED" StandardConstructor::CHANNEL_ABORTED)
    constant("CHIMI_CONSTRUCTOR_INT_PARSE_EMPTY" StandardConstructor::INT_PARSE_EMPTY)
    constant("CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT" StandardConstructor::INT_PARSE_INVALID_DIGIT)
    constant("CHIMI_CONSTRUCTOR_INT_PARSE_OUT_OF_RANGE" StandardConstructor::INT_PARSE_OUT_OF_RANGE)
    constant("CHIMI_CONSTRUCTOR_FLOAT_PARSE_EMPTY" StandardConstructor::FLOAT_PARSE_EMPTY)
    constant("CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID" StandardConstructor::FLOAT_PARSE_INVALID)
    constant("CHIMI_CONSTRUCTOR_FLOAT_PARSE_OUT_OF_RANGE" StandardConstructor::FLOAT_PARSE_OUT_OF_RANGE)
    constant("CHIMI_CONSTRUCTOR_FLOAT_NOT_INTEGER" StandardConstructor::FLOAT_NOT_INTEGER)
    constant("CHIMI_CONSTRUCTOR_FLOAT_OUT_OF_RANGE" StandardConstructor::FLOAT_OUT_OF_RANGE)
    constant("CHIMI_CONSTRUCTOR_OPTION_NONE" StandardConstructor::OPTION_NONE)
    constant("CHIMI_CONSTRUCTOR_DOWN_RETURNED" StandardConstructor::DOWN_RETURNED)
    constant("CHIMI_CONSTRUCTOR_TIMEOUT_TIMED_OUT" StandardConstructor::TIMEOUT_TIMED_OUT)
    constant("CHIMI_CONSTRUCTOR_RESULT_OK" StandardConstructor::RESULT_OK)
    constant("CHIMI_CONSTRUCTOR_RESULT_ERR" StandardConstructor::RESULT_ERR)
    constant("CHIMI_INTRINSIC_JSON_DECODE" Intrinsic::JSON_DECODE)
    constant("CHIMI_INTRINSIC_JSON_ENCODE" Intrinsic::JSON_ENCODE)
    constant("CHIMI_INTRINSIC_JSON_INT" Intrinsic::JSON_INT)
    constant("CHIMI_INTRINSIC_JSON_NUMBER_TO_INT" Intrinsic::JSON_NUMBER_TO_INT)
    constant("CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL" Intrinsic::JSON_NUMBER_TO_DECIMAL)
    constant("CHIMI_INTRINSIC_DECIMAL_SCALED" Intrinsic::DECIMAL_SCALED)
    constant("CHIMI_INTRINSIC_DECIMAL_READ" Intrinsic::DECIMAL_READ)
    constant("CHIMI_INTRINSIC_DECIMAL_TO_TEXT" Intrinsic::DECIMAL_TO_TEXT)
    constant("CHIMI_INTRINSIC_DECIMAL_ADD_CHECKED" Intrinsic::DECIMAL_ADD_CHECKED)
    constant("CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED" Intrinsic::DECIMAL_SUBTRACT_CHECKED)
    constant("CHIMI_INTRINSIC_DECIMAL_MULTIPLY_CHECKED" Intrinsic::DECIMAL_MULTIPLY_CHECKED)
    constant("CHIMI_INTRINSIC_DECIMAL_DIVIDE_ROUNDED" Intrinsic::DECIMAL_DIVIDE_ROUNDED)
    constant("CHIMI_INTRINSIC_DECIMAL_TO_FLOAT" Intrinsic::DECIMAL_TO_FLOAT)
    constant("CHIMI_INTRINSIC_BYTES_FROM" Intrinsic::BYTES_FROM)
    constant("CHIMI_INTRINSIC_BYTES_LENGTH" Intrinsic::BYTES_LENGTH)
    constant("CHIMI_INTRINSIC_BYTES_GET" Intrinsic::BYTES_GET)
    constant("CHIMI_INTRINSIC_BYTES_INDEX_OF" Intrinsic::BYTES_INDEX_OF)
    constant("CHIMI_INTRINSIC_BYTES_CONCAT" Intrinsic::BYTES_CONCAT)
    constant("CHIMI_INTRINSIC_BYTES_SLICE" Intrinsic::BYTES_SLICE)
    constant("CHIMI_INTRINSIC_BYTES_TO_LIST" Intrinsic::BYTES_TO_LIST)
    constant("CHIMI_INTRINSIC_BYTES_FROM_TEXT" Intrinsic::BYTES_FROM_TEXT)
    constant("CHIMI_INTRINSIC_BYTES_TO_TEXT_LOSSY" Intrinsic::BYTES_TO_TEXT_LOSSY)
    constant("CHIMI_INTRINSIC_BYTES_VALID_UTF8_LENGTH" Intrinsic::BYTES_VALID_UTF8_LENGTH)
    constant("CHIMI_INTRINSIC_BYTES_READ_INT_CODE" Intrinsic::BYTES_READ_INT_CODE)
    constant("CHIMI_INTRINSIC_BYTES_INT_BYTES" Intrinsic::BYTES_INT_BYTES)
    constant("CHIMI_INTRINSIC_EMBED_BYTES" Intrinsic::EMBED_BYTES)
    constant("CHIMI_INTRINSIC_HTTP_COUNT_ENDS" Intrinsic::HTTP_COUNT_ENDS)
    constant("CHIMI_INTRINSIC_DURATION_NANOSECONDS" Intrinsic::DURATION_NANOSECONDS)
    constant("CHIMI_INTRINSIC_DURATION_MICROSECONDS" Intrinsic::DURATION_MICROSECONDS)
    constant("CHIMI_INTRINSIC_DURATION_MILLISECONDS" Intrinsic::DURATION_MILLISECONDS)
    constant("CHIMI_INTRINSIC_DURATION_SECONDS" Intrinsic::DURATION_SECONDS)
    constant("CHIMI_INTRINSIC_DURATION_MINUTES" Intrinsic::DURATION_MINUTES)
    constant("CHIMI_INTRINSIC_DURATION_HOURS" Intrinsic::DURATION_HOURS)
    constant("CHIMI_INTRINSIC_DURATION_TO_NANOSECONDS" Intrinsic::DURATION_TO_NANOSECONDS)
    constant("CHIMI_INTRINSIC_DURATION_TO_MICROSECONDS" Intrinsic::DURATION_TO_MICROSECONDS)
    constant("CHIMI_INTRINSIC_DURATION_TO_MILLISECONDS" Intrinsic::DURATION_TO_MILLISECONDS)
    constant("CHIMI_INTRINSIC_DURATION_TO_SECONDS" Intrinsic::DURATION_TO_SECONDS)
    constant("CHIMI_INTRINSIC_DURATION_ADD" Intrinsic::DURATION_ADD)
    constant("CHIMI_INTRINSIC_DURATION_SUBTRACT" Intrinsic::DURATION_SUBTRACT)
    constant("CHIMI_INTRINSIC_DURATION_MULTIPLY" Intrinsic::DURATION_MULTIPLY)
    constant("CHIMI_INTRINSIC_DURATION_DIVIDE" Intrinsic::DURATION_DIVIDE)
    constant("CHIMI_INTRINSIC_DURATION_NEGATE" Intrinsic::DURATION_NEGATE)
    constant("CHIMI_INTRINSIC_DURATION_ABSOLUTE" Intrinsic::DURATION_ABSOLUTE)
    constant("CHIMI_INTRINSIC_DURATION_TO_TEXT" Intrinsic::DURATION_TO_TEXT)
    constant("CHIMI_INTRINSIC_DURATION_PARSE" Intrinsic::DURATION_PARSE)
    constant("CHIMI_INTRINSIC_DEBUG_CRASH" Intrinsic::DEBUG_CRASH)
    constant("CHIMI_INTRINSIC_OPTION_UNWRAP_OR_FAIL" Intrinsic::OPTION_UNWRAP_OR_FAIL)
    constant("CHIMI_INTRINSIC_TEXT_CONCAT" Intrinsic::TEXT_CONCAT)
    constant("CHIMI_INTRINSIC_TEXT_BYTE_LENGTH" Intrinsic::TEXT_BYTE_LENGTH)
    constant("CHIMI_INTRINSIC_TEXT_BYTE_SLICE" Intrinsic::TEXT_BYTE_SLICE)
    constant("CHIMI_INTRINSIC_TEXT_CONTAINS" Intrinsic::TEXT_CONTAINS)
    constant("CHIMI_INTRINSIC_TEXT_STARTS_WITH" Intrinsic::TEXT_STARTS_WITH)
    constant("CHIMI_INTRINSIC_TEXT_ENDS_WITH" Intrinsic::TEXT_ENDS_WITH)
    constant("CHIMI_INTRINSIC_TEXT_SPLIT" Intrinsic::TEXT_SPLIT)
    constant("CHIMI_INTRINSIC_TEXT_REPLACE" Intrinsic::TEXT_REPLACE)
    constant("CHIMI_INTRINSIC_TEXT_TRIM_START" Intrinsic::TEXT_TRIM_START)
    constant("CHIMI_INTRINSIC_TEXT_TRIM_END" Intrinsic::TEXT_TRIM_END)
    constant("CHIMI_INTRINSIC_TEXT_LOWERCASE" Intrinsic::TEXT_LOWERCASE)
    constant("CHIMI_INTRINSIC_TEXT_UPPERCASE" Intrinsic::TEXT_UPPERCASE)
    constant("CHIMI_INTRINSIC_TEXT_GRAPHEMES" Intrinsic::TEXT_GRAPHEMES)
    constant("CHIMI_INTRINSIC_LIST_GET" Intrinsic::LIST_GET)
    constant("CHIMI_INTRINSIC_LIST_PUSH" Intrinsic::LIST_PUSH)
    constant("CHIMI_INTRINSIC_LIST_SLICE" Intrinsic::LIST_SLICE)
    constant("CHIMI_INTRINSIC_LIST_SET" Intrinsic::LIST_SET)
    constant("CHIMI_INTRINSIC_TASK_START" Intrinsic::TASK_START)
    constant("CHIMI_INTRINSIC_TASK_RESULT" Intrinsic::TASK_RESULT)
    constant("CHIMI_INTRINSIC_CLOCK_SLEEP" Intrinsic::CLOCK_SLEEP)
    constant("CHIMI_INTRINSIC_CLOCK_NOW" Intrinsic::CLOCK_NOW)
    constant("CHIMI_INTRINSIC_CLOCK_TIMESTAMP" Intrinsic::CLOCK_TIMESTAMP)
    constant("CHIMI_INTRINSIC_CLOCK_SINCE" Intrinsic::CLOCK_SINCE)
    constant("CHIMI_INTRINSIC_CLOCK_SLEEP_UNTIL" Intrinsic::CLOCK_SLEEP_UNTIL)
    constant("CHIMI_INTRINSIC_CLOCK_ADD" Intrinsic::CLOCK_ADD)
    constant("CHIMI_INTRINSIC_CLOCK_BETWEEN" Intrinsic::CLOCK_BETWEEN)
    constant("CHIMI_INTRINSIC_CLOCK_TIMESTAMP_TO_TEXT" Intrinsic::CLOCK_TIMESTAMP_TO_TEXT)
    constant("CHIMI_INTRINSIC_CLOCK_UNIX_NANOSECONDS" Intrinsic::CLOCK_UNIX_NANOSECONDS)
    constant("CHIMI_INTRINSIC_CLOCK_FROM_UNIX_NANOSECONDS" Intrinsic::CLOCK_FROM_UNIX_NANOSECONDS)
    constant("CHIMI_INTRINSIC_CLOCK_TIMESTAMP_BETWEEN" Intrinsic::CLOCK_TIMESTAMP_BETWEEN)
    constant("CHIMI_INTRINSIC_CLOCK_STARTING_AT" Intrinsic::CLOCK_STARTING_AT)
    constant("CHIMI_INTRINSIC_TASK_TIMEOUT" Intrinsic::TASK_TIMEOUT)
    constant("CHIMI_INTRINSIC_TASK_DEADLINE" Intrinsic::TASK_DEADLINE)
    constant("CHIMI_INTRINSIC_SELECT_RECEIVE" Intrinsic::SELECT_RECEIVE)
    constant("CHIMI_INTRINSIC_SELECT_INBOX" Intrinsic::SELECT_INBOX)
    constant("CHIMI_INTRINSIC_SELECT_TASK" Intrinsic::SELECT_TASK)
    constant("CHIMI_INTRINSIC_SELECT_AFTER" Intrinsic::SELECT_AFTER)
    constant("CHIMI_INTRINSIC_SELECT_AT" Intrinsic::SELECT_AT)
    constant("CHIMI_INTRINSIC_SELECT_FIRST" Intrinsic::SELECT_FIRST)
    constant("CHIMI_INTRINSIC_SELECT_WATCH" Intrinsic::SELECT_WATCH)
    constant("CHIMI_INTRINSIC_PROCESS_SPEC" Intrinsic::PROCESS_SPEC)
    constant("CHIMI_INTRINSIC_PROCESS_REGISTER" Intrinsic::PROCESS_REGISTER)
    constant("CHIMI_INTRINSIC_PROCESS_HANDOFF_SPEC" Intrinsic::PROCESS_HANDOFF_SPEC)
    constant("CHIMI_INTRINSIC_SUPERVISOR_PROCESS" Intrinsic::SUPERVISOR_PROCESS)
    constant("CHIMI_INTRINSIC_SUPERVISOR_SUPERVISOR" Intrinsic::SUPERVISOR_SUPERVISOR)
    constant("CHIMI_INTRINSIC_SUPERVISOR_POLICY" Intrinsic::SUPERVISOR_POLICY)
    constant("CHIMI_INTRINSIC_SUPERVISOR_RESTARTS" Intrinsic::SUPERVISOR_RESTARTS)
    constant("CHIMI_INTRINSIC_PROCESS_RECEIVE" Intrinsic::PROCESS_RECEIVE)
    constant("CHIMI_INTRINSIC_PROCESS_OWN_ADDRESS" Intrinsic::PROCESS_OWN_ADDRESS)
    constant("CHIMI_INTRINSIC_PROCESS_QUEUED" Intrinsic::PROCESS_QUEUED)
    constant("CHIMI_INTRINSIC_PROCESS_CAPACITY" Intrinsic::PROCESS_CAPACITY)
    constant("CHIMI_INTRINSIC_PROCESS_UNMONITOR" Intrinsic::PROCESS_UNMONITOR)
    constant("CHIMI_INTRINSIC_SUPERVISOR_ADDRESS" Intrinsic::SUPERVISOR_ADDRESS)
    constant("CHIMI_INTRINSIC_SUPERVISOR_REMOVE_PROCESS" Intrinsic::SUPERVISOR_REMOVE_PROCESS)
    constant("CHIMI_INTRINSIC_SUPERVISOR_REMOVE_SUPERVISOR" Intrinsic::SUPERVISOR_REMOVE_SUPERVISOR)
    constant("CHIMI_INTRINSIC_SUPERVISOR_STOP" Intrinsic::SUPERVISOR_STOP)
    constant("CHIMI_INTRINSIC_SUPERVISOR_WAIT" Intrinsic::SUPERVISOR_WAIT)
    constant("CHIMI_INTRINSIC_TASK_ALL" Intrinsic::TASK_ALL)
    constant("CHIMI_INTRINSIC_TASK_RESULTS" Intrinsic::TASK_RESULTS)
    constant("CHIMI_INTRINSIC_TASK_RACE" Intrinsic::TASK_RACE)
    constant("CHIMI_INTRINSIC_TASK_CANCEL" Intrinsic::TASK_CANCEL)
    constant("CHIMI_INTRINSIC_CHANNEL_SEND" Intrinsic::CHANNEL_SEND)
    constant("CHIMI_INTRINSIC_CHANNEL_RECEIVE" Intrinsic::CHANNEL_RECEIVE)
    constant("CHIMI_INTRINSIC_CHANNEL_CLOSE" Intrinsic::CHANNEL_CLOSE)
    constant("CHIMI_INTRINSIC_CHANNEL_ABORT" Intrinsic::CHANNEL_ABORT)
    constant("CHIMI_INTRINSIC_CHANNEL_BOUNDED" Intrinsic::CHANNEL_BOUNDED)
    constant("CHIMI_INTRINSIC_CHANNEL_ADD_RECEIVER" Intrinsic::CHANNEL_ADD_RECEIVER)
    constant("CHIMI_INTRINSIC_BROADCAST_BOUNDED" Intrinsic::BROADCAST_BOUNDED)
    constant("CHIMI_INTRINSIC_BROADCAST_PUBLISH" Intrinsic::BROADCAST_PUBLISH)
    constant("CHIMI_INTRINSIC_BROADCAST_SUBSCRIBE" Intrinsic::BROADCAST_SUBSCRIBE)
    constant("CHIMI_INTRINSIC_WATCH_STARTING_WITH" Intrinsic::WATCH_STARTING_WITH)
    constant("CHIMI_INTRINSIC_WATCH_PUBLISH" Intrinsic::WATCH_PUBLISH)
    constant("CHIMI_INTRINSIC_WATCH_SUBSCRIBE" Intrinsic::WATCH_SUBSCRIBE)
    constant("CHIMI_INTRINSIC_WATCH_RECEIVE" Intrinsic::WATCH_RECEIVE)
    constant("CHIMI_INTRINSIC_CAUSE_CONTEXT" Intrinsic::CAUSE_CONTEXT)
    constant("CHIMI_INTRINSIC_CAUSE_OF" Intrinsic::CAUSE_OF)
    constant("CHIMI_INTRINSIC_CAUSE_KIND_CODE" Intrinsic::CAUSE_KIND_CODE)
    constant("CHIMI_INTRINSIC_CAUSE_SUMMARY" Intrinsic::CAUSE_SUMMARY)
    constant("CHIMI_INTRINSIC_CAUSE_FRAMES" Intrinsic::CAUSE_FRAMES)
    constant("CHIMI_INTRINSIC_CAUSE_CAUSED_BY" Intrinsic::CAUSE_CAUSED_BY)
    constant("CHIMI_INTRINSIC_CAUSE_SUPPRESSED" Intrinsic::CAUSE_SUPPRESSED)
    constant("CHIMI_INTRINSIC_CAUSE_REASON_CODE" Intrinsic::CAUSE_REASON_CODE)
    constant("CHIMI_INTRINSIC_CAUSE_TO_TEXT" Intrinsic::CAUSE_TO_TEXT)
    constant("CHIMI_INTRINSIC_RESOURCE_SCOPED" Intrinsic::RESOURCE_SCOPED)
    constant("CHIMI_INTRINSIC_TEST_CAPTURE_DEFECT" Intrinsic::TEST_CAPTURE_DEFECT)
    constant("CHIMI_INTRINSIC_TEST_ASSERT_EQUAL" Intrinsic::TEST_ASSERT_EQUAL)
    constant("CHIMI_INTRINSIC_TEST_ASSERT_NOT_EQUAL" Intrinsic::TEST_ASSERT_NOT_EQUAL)
    constant("CHIMI_INTRINSIC_TEST_ASSERT" Intrinsic::TEST_ASSERT)
    constant("CHIMI_INTRINSIC_TEST_FAIL" Intrinsic::TEST_FAIL)
    constant("CHIMI_INTRINSIC_TEST_RECORDER" Intrinsic::TEST_RECORDER)
    constant("CHIMI_INTRINSIC_TEST_RECORD" Intrinsic::TEST_RECORD)
    constant("CHIMI_INTRINSIC_TEST_RECORDED" Intrinsic::TEST_RECORDED)
    constant("CHIMI_INTRINSIC_TEST_SCRIPT" Intrinsic::TEST_SCRIPT)
    constant("CHIMI_INTRINSIC_TEST_NEXT" Intrinsic::TEST_NEXT)
    constant("CHIMI_INTRINSIC_TEST_MAILBOX" Intrinsic::TEST_MAILBOX)
    constant("CHIMI_INTRINSIC_TEST_RECEIVED" Intrinsic::TEST_RECEIVED)
    constant("CHIMI_INTRINSIC_IO_PATH" Intrinsic::IO_PATH)
    constant("CHIMI_INTRINSIC_LIST_LENGTH" Intrinsic::LIST_LENGTH)
    constant("CHIMI_INTRINSIC_TEXT_APPEND" Intrinsic::TEXT_APPEND)
    constant("CHIMI_INTRINSIC_TEXT_EQUAL" Intrinsic::TEXT_EQUAL)
    constant("CHIMI_INTRINSIC_INT_PARSE" Intrinsic::INT_PARSE)
    constant("CHIMI_INTRINSIC_INT_TO_TEXT" Intrinsic::INT_TO_TEXT)
    constant("CHIMI_INTRINSIC_INT_ADD_CHECKED" Intrinsic::INT_ADD_CHECKED)
    constant("CHIMI_INTRINSIC_INT_SUBTRACT_CHECKED" Intrinsic::INT_SUBTRACT_CHECKED)
    constant("CHIMI_INTRINSIC_INT_MULTIPLY_CHECKED" Intrinsic::INT_MULTIPLY_CHECKED)
    constant("CHIMI_INTRINSIC_INT_DIVIDE_CHECKED" Intrinsic::INT_DIVIDE_CHECKED)
    constant("CHIMI_INTRINSIC_INT_REMAINDER_CHECKED" Intrinsic::INT_REMAINDER_CHECKED)
    constant("CHIMI_INTRINSIC_INT_MODULO" Intrinsic::INT_MODULO)
    constant("CHIMI_INTRINSIC_FLOAT_FROM_INT" Intrinsic::FLOAT_FROM_INT)
    constant("CHIMI_INTRINSIC_FLOAT_TRUNCATE" Intrinsic::FLOAT_TRUNCATE)
    constant("CHIMI_INTRINSIC_FLOAT_FLOOR" Intrinsic::FLOAT_FLOOR)
    constant("CHIMI_INTRINSIC_FLOAT_CEILING" Intrinsic::FLOAT_CEILING)
    constant("CHIMI_INTRINSIC_FLOAT_ROUND" Intrinsic::FLOAT_ROUND)
    constant("CHIMI_INTRINSIC_FLOAT_TO_INT" Intrinsic::FLOAT_TO_INT)
    constant("CHIMI_INTRINSIC_FLOAT_TO_TEXT" Intrinsic::FLOAT_TO_TEXT)
    constant("CHIMI_INTRINSIC_FLOAT_PARSE" Intrinsic::FLOAT_PARSE)
    constant("CHIMI_INTRINSIC_DEBUG_RENDER" Intrinsic::DEBUG_RENDER)
    constant("CHIMI_INTRINSIC_DEBUG_RENDER_INDENTED" Intrinsic::DEBUG_RENDER_INDENTED)
    constant("CHIMI_INTRINSIC_RANGE_START" Intrinsic::RANGE_START)
    constant("CHIMI_INTRINSIC_RANGE_END" Intrinsic::RANGE_END)
    constant("CHIMI_INTRINSIC_RANGE_LENGTH" Intrinsic::RANGE_LENGTH)
    constant("CHIMI_INTRINSIC_RANGE_CONTAINS" Intrinsic::RANGE_CONTAINS)
    constant("CHIMI_INTRINSIC_PATH_FROM" Intrinsic::PATH_FROM)
    constant("CHIMI_INTRINSIC_PATH_TO_TEXT" Intrinsic::PATH_TO_TEXT)
    constant("CHIMI_INTRINSIC_ENV_HOST_VARIABLES" Intrinsic::ENV_HOST_VARIABLES)
    constant("CHIMI_INTRINSIC_ENV_HOST_CURRENT_DIRECTORY" Intrinsic::ENV_HOST_CURRENT_DIRECTORY)
    constant("CHIMI_INTRINSIC_FS_HOST_OPEN" Intrinsic::FS_HOST_OPEN)
    constant("CHIMI_INTRINSIC_FS_HOST_CREATE" Intrinsic::FS_HOST_CREATE)
    constant("CHIMI_INTRINSIC_FS_HOST_READ" Intrinsic::FS_HOST_READ)
    constant("CHIMI_INTRINSIC_FS_HOST_WRITE" Intrinsic::FS_HOST_WRITE)
    constant("CHIMI_INTRINSIC_FS_HOST_CLOSE" Intrinsic::FS_HOST_CLOSE)
    constant("CHIMI_INTRINSIC_FS_HOST_METADATA" Intrinsic::FS_HOST_METADATA)
    constant("CHIMI_INTRINSIC_FS_HOST_LIST" Intrinsic::FS_HOST_LIST)
    constant("CHIMI_INTRINSIC_FS_HOST_CREATE_DIRECTORY" Intrinsic::FS_HOST_CREATE_DIRECTORY)
    constant("CHIMI_INTRINSIC_FS_HOST_REMOVE" Intrinsic::FS_HOST_REMOVE)
    constant("CHIMI_INTRINSIC_FS_HOST_RENAME" Intrinsic::FS_HOST_RENAME)
    constant("CHIMI_INTRINSIC_FS_HOST_IS_DIRECTORY" Intrinsic::FS_HOST_IS_DIRECTORY)
    constant("CHIMI_INTRINSIC_FS_HOST_TEMPORARY_DIRECTORY" Intrinsic::FS_HOST_TEMPORARY_DIRECTORY)
    constant("CHIMI_INTRINSIC_FS_TEST_ROOT" Intrinsic::FS_TEST_ROOT)
    constant("CHIMI_ADDRESS_TEST" ADDRESS_TEST)
    constant("CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY" StandardConstructor::DURATION_PARSE_EMPTY)
    constant("CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID" StandardConstructor::DURATION_PARSE_INVALID)
    constant("CHIMI_CONSTRUCTOR_PROCESS_SEND_FULL" StandardConstructor::PROCESS_SEND_FULL)
    constant("CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE" StandardConstructor::DURATION_PARSE_OUT_OF_RANGE)
    constant("CHIMI_CONSTRUCTOR_JSON_NULL" StandardConstructor::JSON_NULL)
    constant("CHIMI_CONSTRUCTOR_JSON_BOOLEAN" StandardConstructor::JSON_BOOLEAN)
    constant("CHIMI_CONSTRUCTOR_JSON_NUMBER" StandardConstructor::JSON_NUMBER)
    constant("CHIMI_CONSTRUCTOR_JSON_TEXT" StandardConstructor::JSON_TEXT)
    constant("CHIMI_CONSTRUCTOR_JSON_ARRAY" StandardConstructor::JSON_ARRAY)
    constant("CHIMI_CONSTRUCTOR_JSON_OBJECT" StandardConstructor::JSON_OBJECT)
    constant("CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON" StandardConstructor::DECODE_INVALID_JSON)
    constant("CHIMI_CONSTRUCTOR_JSON_NOT_INTEGER" StandardConstructor::JSON_NOT_INTEGER)
    constant("CHIMI_CONSTRUCTOR_JSON_OUT_OF_RANGE" StandardConstructor::JSON_OUT_OF_RANGE)
    size("MapNode" MapNode)
    field("MapNode" MapNode, "key", key)
    field("MapNode" MapNode, "value", value)
    field("MapNode" MapNode, "left", left)
    field("MapNode" MapNode, "right", right)
    field("MapNode" MapNode, "height", height)
    size("ChimiEnvironment" Environment)
    field("ChimiEnvironment" Environment, "locals", locals)
    field("ChimiEnvironment" Environment, "requirements", requirements)
    field("ChimiEnvironment" Environment, "local_count", local_count)
    size("ChimiCallable" Callable)
    field("ChimiCallable" Callable, "kind", kind)
    field("ChimiCallable" Callable, "symbol", symbol)
    field("ChimiCallable" Callable, "body", body)
    field("ChimiCallable" Callable, "parameters_offset", parameters_offset)
    field("ChimiCallable" Callable, "parameters_length", parameters_length)
    field("ChimiCallable" Callable, "local_count", local_count)
    field("ChimiCallable" Callable, "environment", environment)
    field("ChimiCallable" Callable, "inner", inner)
    field("ChimiCallable" Callable, "requirement", requirement)
    field("ChimiCallable" Callable, "interface_symbol", interface_symbol)
    field("ChimiCallable" Callable, "method", method)
    field("ChimiCallable" Callable, "name", name)
    field("ChimiCallable" Callable, "intrinsic", intrinsic)
    field("ChimiCallable" Callable, "host", host)
    field("ChimiCallable" Callable, "shift", shift)
    size("ChimiControl" Control)
    field("ChimiControl" Control, "kind", kind)
    field("ChimiControl" Control, "value", value)
    field("ChimiControl" Control, "message", message)
    field("ChimiControl" Control, "node", node)
    field("ChimiControl" Control, "cause", cause)
    size("ChimiValue" Value)
    align("ChimiValue" Value)
    field("ChimiValue" Value, "kind", kind)
    field("ChimiValue" Value, "as", payload)
    field("ChimiValue" Value, "as.integer", payload.integer)
    field("ChimiValue" Value, "as.floating", payload.floating)
    field("ChimiValue" Value, "as.boolean", payload.boolean)
    field("ChimiValue" Value, "as.callable", payload.callable)
    field("ChimiValue" Value, "as.map", payload.map)
    field("ChimiValue" Value, "as.json_number", payload.json_number)
    field("ChimiValue" Value, "as.task", payload.task)
    field("ChimiValue" Value, "as.text", payload.text)
    field("ChimiValue" Value, "as.text.bytes", payload.text.bytes)
    field("ChimiValue" Value, "as.text.length", payload.text.length)
    field("ChimiValue" Value, "as.list.vector.root", payload.list.vector.root)
    field("ChimiValue" Value, "as.list.vector.tail", payload.list.vector.tail)
    field("ChimiValue" Value, "as.list.vector.length", payload.list.vector.length)
    field("ChimiValue" Value, "as.list.start", payload.list.start)
    field("ChimiValue" Value, "as.list.length", payload.list.length)
    field("ChimiValue" Value, "as.record.fields", payload.record.fields)
    field("ChimiValue" Value, "as.record.length", payload.record.length)
    field("ChimiValue" Value, "as.constructed.constructor", payload.constructed.constructor)
    field("ChimiValue" Value, "as.constructed.type_name", payload.constructed.type_name)
    field("ChimiValue" Value, "as.constructed.variant_name", payload.constructed.variant_name)
    field("ChimiValue" Value, "as.constructed.fields", payload.constructed.fields)
    field("ChimiValue" Value, "as.constructed.length", payload.constructed.length)
    field("ChimiValue" Value, "as.range.start", payload.range.start)
    field("ChimiValue" Value, "as.range.end", payload.range.end)
    field("ChimiValue" Value, "as.capability.channel", payload.capability.channel)
    field("ChimiValue" Value, "as.capability.owner", payload.capability.owner)
    field("ChimiValue" Value, "as.capability.token", payload.capability.token)
    field("ChimiValue" Value, "as.address.kind", payload.address.kind)
    field("ChimiValue" Value, "as.address.target", payload.address.target)
    field("ChimiValue" Value, "as.managed.supervisor", payload.managed.supervisor)
    field("ChimiValue" Value, "as.managed.membership", payload.managed.membership)
    field("ChimiValue" Value, "as.decimal", payload.decimal)
    field("ChimiValue" Value, "as.handle.slot", payload.handle.slot)
    field("ChimiValue" Value, "as.handle.generation", payload.handle.generation)
    field("ChimiValue" Value, "as.handle.name", payload.handle.name)

    size("AllocationHeader" AllocationHeader)
    align("AllocationHeader" AllocationHeader)
    field("AllocationHeader" AllocationHeader, "next", next)
    field("AllocationHeader" AllocationHeader, "area", area)
    field("AllocationHeader" AllocationHeader, "bytes", bytes)
    field("AllocationHeader" AllocationHeader, "mark_epoch", mark_epoch)
    field("AllocationHeader" AllocationHeader, "kind", kind)
    field("AllocationHeader" AllocationHeader, "in_chunk", in_chunk)
    size("YoungChunk" YoungChunk)
    align("YoungChunk" YoungChunk)
    field("YoungChunk" YoungChunk, "next", next)
    field("YoungChunk" YoungChunk, "used", used)
    constant("offsetof(YoungChunk, data)" offset_of!(YoungChunk, data))
    size("AllocationArea" AllocationArea)
    field("AllocationArea" AllocationArea, "role", role)
    field("AllocationArea" AllocationArea, "heap", heap)
    field("AllocationArea" AllocationArea, "scope", scope)
    field("AllocationArea" AllocationArea, "allocations", allocations)
    field("AllocationArea" AllocationArea, "bytes", bytes)
    field("AllocationArea" AllocationArea, "count", count)
    field("AllocationArea" AllocationArea, "next", next)
    size("AllocationScope" AllocationScope)
    field("AllocationScope" AllocationScope, "parent", parent)
    field("AllocationScope" AllocationScope, "heap", heap)
    field("AllocationScope" AllocationScope, "young", young)
    field("AllocationScope" AllocationScope, "adopted", adopted)
    field("AllocationScope" AllocationScope, "survivors", survivors)
    field("AllocationScope" AllocationScope, "full_baseline", full_baseline)
    field("AllocationScope" AllocationScope, "has_baseline", has_baseline)
    field("AllocationScope" AllocationScope, "first_chunk", first_chunk)
    field("AllocationScope" AllocationScope, "last_chunk", last_chunk)
    field("AllocationScope" AllocationScope, "bump", bump)
    field("AllocationScope" AllocationScope, "bump_end", bump_end)
    field("AllocationScope" AllocationScope, "pending_bytes", pending_bytes)
    field("AllocationScope" AllocationScope, "pending_count", pending_count)
    field("AllocationScope" AllocationScope, "links_young", links_young)
    field("AllocationScope" AllocationScope, "live_bytes", live_bytes)
    field("AllocationScope" AllocationScope, "peak_live_bytes", peak_live_bytes)
    field("AllocationScope" AllocationScope, "total_allocated_bytes", total_allocated_bytes)
    field("AllocationScope" AllocationScope, "allocated_bytes_by_kind", allocated_bytes_by_kind)
    field("AllocationScope" AllocationScope, "collections", collections)
    field("AllocationScope" AllocationScope, "full_collections", full_collections)
    field("AllocationScope" AllocationScope, "scanned_allocations", scanned_allocations)
    field("AllocationScope" AllocationScope, "promoted_bytes", promoted_bytes)
    size("RuntimeHeap" RuntimeHeap)
    field("RuntimeHeap" RuntimeHeap, "owner", owner)
    field("RuntimeHeap" RuntimeHeap, "profiled_node_plus_one", profiled_node_plus_one)
    field("RuntimeHeap" RuntimeHeap, "live_bytes", live_bytes)
    field("RuntimeHeap" RuntimeHeap, "peak_live_bytes", peak_live_bytes)
    field("RuntimeHeap" RuntimeHeap, "innermost", innermost)
    field("RuntimeHeap" RuntimeHeap, "base", base)
    field("RuntimeHeap" RuntimeHeap, "adopted", adopted)
    size("HeapTotals" HeapTotals)
    field("HeapTotals" HeapTotals, "live_allocations", live_allocations)
    field("HeapTotals" HeapTotals, "footprint_bytes", footprint_bytes)
    field("HeapTotals" HeapTotals, "peak_footprint_bytes", peak_footprint_bytes)
    field("HeapTotals" HeapTotals, "live_fragments", live_fragments)
    field("HeapTotals" HeapTotals, "fragment_footprint_bytes", fragment_footprint_bytes)
    field("HeapTotals" HeapTotals, "peak_fragment_footprint_bytes", peak_fragment_footprint_bytes)
    field("HeapTotals" HeapTotals, "live_heaps", live_heaps)
    field("HeapTotals" HeapTotals, "cached_bytes", cached_bytes)
    size("ScopeTotals" ScopeTotals)
    field("ScopeTotals" ScopeTotals, "total_allocated_bytes", total_allocated_bytes)
    field("ScopeTotals" ScopeTotals, "allocated_bytes_by_kind", allocated_bytes_by_kind)
    field("ScopeTotals" ScopeTotals, "peak_live_bytes", peak_live_bytes)
    field("ScopeTotals" ScopeTotals, "retained_bytes", retained_bytes)
    field("ScopeTotals" ScopeTotals, "collections", collections)
    field("ScopeTotals" ScopeTotals, "full_collections", full_collections)
    field("ScopeTotals" ScopeTotals, "scanned_allocations", scanned_allocations)
    size("FiberContext" FiberContext)
    field("FiberContext" FiberContext, "heap", heap)
    field("FiberContext" FiberContext, "target", target)
    size("SmallBlock" SmallBlock)
    field("SmallBlock" SmallBlock, "next", next)
    size("SmallBlockCache" SmallBlockCache)
    field("SmallBlockCache" SmallBlockCache, "lists", lists)
    field("SmallBlockCache" SmallBlockCache, "counts", counts)
    size("PointerStack" PointerStack)
    field("PointerStack" PointerStack, "items", items)
    field("PointerStack" PointerStack, "length", length)
    field("PointerStack" PointerStack, "capacity", capacity)
    size("Carrier" Carrier)
    field("Carrier" Carrier, "fiber", fiber)
    field("Carrier" Carrier, "heap_totals", heap_totals)
    field("Carrier" Carrier, "pending_footprint_bytes", pending_footprint_bytes)
    field("Carrier" Carrier, "folded_footprint_bytes", folded_footprint_bytes)
    field("Carrier" Carrier, "folded_fragment_footprint_bytes", folded_fragment_footprint_bytes)
    field("Carrier" Carrier, "unscoped_allocated_bytes", unscoped_allocated_bytes)
    field("Carrier" Carrier, "capture_transfer_probes", capture_transfer_probes)
    field("Carrier" Carrier, "process_heap_peak_value_bytes", process_heap_peak_value_bytes)
    field("Carrier" Carrier, "copy_counting", copy_counting)
    field("Carrier" Carrier, "copied_values", copied_values)
    field("Carrier" Carrier, "copied_bytes", copied_bytes)
    field("Carrier" Carrier, "fragment_copy_foreign_owner", fragment_copy_foreign_owner)
    field("Carrier" Carrier, "small_blocks", small_blocks)
    field("Carrier" Carrier, "young_chunk_cache", young_chunk_cache)
    field("Carrier" Carrier, "young_chunk_cache_count", young_chunk_cache_count)
    field("Carrier" Carrier, "collecting_scope", collecting_scope)
    field("Carrier" Carrier, "collecting_full", collecting_full)
    field("Carrier" Carrier, "collecting_foreign", collecting_foreign)
    field("Carrier" Carrier, "collecting_epoch", collecting_epoch)
    field("Carrier" Carrier, "next_epoch", next_epoch)
    field("Carrier" Carrier, "end_epoch", end_epoch)
    field("Carrier" Carrier, "next_span", next_span)
    field("Carrier" Carrier, "end_span", end_span)
    field("Carrier" Carrier, "mark_stack", mark_stack)
    field("Carrier" Carrier, "copy_stack", copy_stack)
    field("Carrier" Carrier, "copy_pending", copy_pending)
    field("Carrier" Carrier, "fs_read_chunk", fs_read_chunk)
    field("Carrier" Carrier, "scope_totals", scope_totals)
    field("Carrier" Carrier, "host_stack_limit", host_stack_limit)
    field("Carrier" Carrier, "bound", bound)
    field("Carrier" Carrier, "console_lock_held", console_lock_held)
    size("ProfileRow" ProfileRow)
    field("ProfileRow" ProfileRow, "operation", operation)
    field("ProfileRow" ProfileRow, "evaluations", evaluations)
    field("ProfileRow" ProfileRow, "allocations", allocations)
    field("ProfileRow" ProfileRow, "allocated_bytes", allocated_bytes)
    size("RuntimeFragment" RuntimeFragment)
    field("RuntimeFragment" RuntimeFragment, "area", area)
    field("RuntimeFragment" RuntimeFragment, "root_count", root_count)
    field("RuntimeFragment" RuntimeFragment, "roots", roots)
};

/// The opening and closing lines of the generated block in `runtime.c`.
pub const C_BEGIN: &str = "/* BEGIN generated layout asserts: chimi-runtime heap/layout.rs LAYOUT.";
pub const C_END: &str = "/* END generated layout asserts. */";

/// Writes the block `runtime.c` carries, `C_BEGIN` through `C_END`, one
/// `_Static_assert` per [`LAYOUT`] fact.
pub fn c_asserts(out: &mut impl core::fmt::Write) -> core::fmt::Result {
    writeln!(out, "{C_BEGIN}")?;
    writeln!(out, "   Regenerate with CHIMI_LAYOUT_BLESS=1 cargo test -p chimi --test runtime_layout. */")?;
    writeln!(
        out,
        "#define CHIMI_LAYOUT_CLASS(x) _Generic((x), _Bool: 3, float: 4, double: 4, \\\n  default: ((__typeof__(x))-1 < (__typeof__(x))1 ? 1 : 2))"
    )?;
    let mut check = |c: &dyn core::fmt::Display, value: usize| {
        writeln!(out, "_Static_assert({c} == {value}, \"layout: {c}\");")
    };
    for fact in LAYOUT {
        match *fact {
            Fact::Value { c, value } => check(&c, value)?,
            Fact::Field { ty, field, offset, size, class } => {
                check(&format_args!("offsetof({ty}, {field})"), offset)?;
                check(&format_args!("sizeof((({ty} *)0)->{field})"), size)?;
                if class != Class::Compound {
                    check(&format_args!("CHIMI_LAYOUT_CLASS((({ty} *)0)->{field})"), class as usize)?;
                }
            }
        }
    }
    writeln!(out, "#undef CHIMI_LAYOUT_CLASS")?;
    writeln!(out, "{C_END}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fact_names_a_distinct_expression() {
        let mut names: Vec<_> = LAYOUT
            .iter()
            .map(|fact| match *fact {
                Fact::Value { c, .. } => (c, ""),
                Fact::Field { ty, field, .. } => (ty, field),
            })
            .collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    #[test]
    fn the_value_layout_is_the_one_the_collector_expects() {
        // Spot checks that do not depend on the C side, so a mistaken mirror
        // cannot bless itself: a value is a tag and a 40-byte payload.
        assert_eq!(size_of::<Value>(), 48);
        assert_eq!(offset_of!(Value, payload), 8);
        assert_eq!(size_of::<AllocationHeader>(), 40);
        assert_eq!(offset_of!(YoungChunk, data), 16);
        assert_eq!(YOUNG_CHUNK_CAPACITY, 16 * 1024 - 16);
    }
}
