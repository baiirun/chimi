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

/// The `CHIMI_INTRINSIC_*` ids (runtime.h) of the intrinsics Rust serves.
pub struct Intrinsic;

impl Intrinsic {
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
}

/// The `CHIMI_CONSTRUCTOR_*` standard constructor ids (runtime.h) Rust builds.
pub struct StandardConstructor;

impl StandardConstructor {
    pub const OPTION_SOME: u32 = 6;
    pub const OPTION_NONE: u32 = 7;
    pub const JSON_NULL: u32 = 8;
    pub const JSON_BOOLEAN: u32 = 9;
    pub const JSON_NUMBER: u32 = 10;
    pub const JSON_TEXT: u32 = 11;
    pub const JSON_ARRAY: u32 = 12;
    pub const JSON_OBJECT: u32 = 13;
    pub const DECODE_INVALID_JSON: u32 = 14;
    pub const JSON_NOT_INTEGER: u32 = 15;
    pub const JSON_OUT_OF_RANGE: u32 = 16;
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
    AllocationArea, FiberContext, HeapTotals, ScopeTotals, SmallBlockCache, PointerStack);

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
    constant("CHIMI_CONSTRUCTOR_OPTION_NONE" StandardConstructor::OPTION_NONE)
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
