//! The evaluator of native programs (CHI-305 step 5, phase 7): the runtime
//! half of each core node, `chimi_rt_<operation>`, which the compiler's node
//! functions call when they do not lower a node themselves; `run_node`, which
//! calls a child's node function; the span and profile row each node takes;
//! and `invoke` for the calls a program defines. The intrinsics, host
//! functions and C bindings `invoke` reaches stay behind
//! `chimi_runtime_invoke_native_v1` in `runtime.c`, and so do the loop
//! nodes, whose passes talk to the scheduler.
//!
//! Core nodes share their incoming environment because ordinary evaluation
//! only reads it. Sequence and provision clone before binding locals or
//! requirements, and match arms clone before binding patterns. A node that
//! writes either table must clone first in the same way instead of writing
//! through the shared pointer.
//!
//! Every message, allocation and allocation kind is the C's, in the C's
//! order, so outputs and memory reports stay as they were.

use core::ffi::{CStr, c_char, c_void};
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering::Relaxed};

use crate::decimal::Decimal;
use crate::heap::allocator::{current_carrier, one_thread_runs_fibers, profile_rows};
use crate::heap::compare::compare;
use crate::heap::layout::{
    AllocationKind, Arm, Callable, CallableKind, Constructor, Control, ControlKind, Core, Environment, Field,
    Function, NamedNode, Node, Pattern, Runtime, Step, Value, ValueKind,
};
use crate::heap::list;
use crate::heap::values::{
    allocate_kind, clone_environment, clone_value_slots, decimal_value, environment, float_value, integer_value,
    new_control, new_environment, new_value, new_value_slots, pointer_value, range_value, requirement_slots,
    text_value,
};
use crate::intrinsics::causes::{cause_derived, control_with_cause, matched_cause_value, with_stored_cause};
use crate::intrinsics::decimals;
use crate::intrinsics::message::Message;
use crate::intrinsics::standard::{boolean_value, defect, defect_with, failure, string, success, symbol, void_value};
use crate::telemetry::{chimi_trace_dependency_v1, chimi_trace_event_v1};

/// `ChimiNodeFn`: a node's compiled function.
type NodeFn = unsafe extern "C" fn(*mut Runtime, *mut Environment, u64) -> *mut Control;

unsafe extern "C" {
    static CHIMI_NODE_FUNCTIONS: [NodeFn; 0];
    static CHIMI_NODES: [Node; 0];
    static CHIMI_IDS: [u32; 0];
    static CHIMI_STRING_LENGTHS: [u32; 0];
    static CHIMI_NAMED_NODES: [NamedNode; 0];
    static CHIMI_STEPS: [Step; 0];
    static CHIMI_ARMS: [Arm; 0];
    static CHIMI_PATTERNS: [Pattern; 0];
    static CHIMI_FUNCTIONS: [Function; 0];
    static CHIMI_FUNCTION_COUNT: u32;
    static CHIMI_SYMBOL_COUNT: u32;
    static CHIMI_CLOCK_REQUIREMENT_SLOT: u32;
    static CHIMI_CONSTRUCTORS: [Constructor; 0];
    /// The module constants, decoded before the entry runs.
    static chimi_runtime_constant_values: AtomicPtr<*mut Value>;
    /// The runner's execution clock provider, as the `clock` need's value.
    fn chimi_runtime_execution_clock_value_v1() -> *mut Value;
    /// `invoke` for an intrinsic, a host function or a C binding.
    fn chimi_runtime_invoke_native_v1(
        runtime: *mut Runtime,
        callable: *mut Callable,
        arguments: *mut *mut Value,
        argument_count: u32,
        span: u64,
        call_node: u32,
        environment: *mut Environment,
    ) -> *mut Control;
    /// `fmod` from the C library, which the `libc` crate does not declare.
    fn fmod(x: f64, y: f64) -> f64;
}

/// Span ids a carrier takes from the runtime's counter at once when several
/// carriers run. Traced modes take a span per node, and one shared fetch-add
/// per node made a traced counting loop slower on four carriers than on one
/// (step 3 PR 6a review).
const SPAN_BLOCK: u64 = 4096;

/// Most matches have one or two subjects, so their values live on the stack.
const MATCH_STACK_SUBJECTS: usize = 8;

/// # Safety
/// `id` is a node of the program.
#[inline(always)]
unsafe fn node(id: u32) -> &'static Node {
    // SAFETY: the caller's contract.
    unsafe { &*(&raw const CHIMI_NODES).cast::<Node>().add(id as usize) }
}

/// # Safety
/// `index` is inside the program's id table.
#[inline(always)]
unsafe fn id(index: u32) -> u32 {
    // SAFETY: the caller's contract.
    unsafe { *(&raw const CHIMI_IDS).cast::<u32>().add(index as usize) }
}

/// # Safety
/// `index` is inside the table `table` is the start of.
#[inline(always)]
unsafe fn entry<T>(table: *const [T; 0], index: u32) -> &'static T {
    // SAFETY: the caller's contract.
    unsafe { &*table.cast::<T>().add(index as usize) }
}

/// The environment slot of the symbol `symbol_index`.
///
/// # Safety
/// `symbol_index` is a symbol of the program.
#[inline(always)]
unsafe fn slot(symbol_index: u32) -> usize {
    // SAFETY: the caller's contract.
    unsafe { symbol(symbol_index).environment_slot as usize }
}

/// The string `id`'s bytes, which may hold U+0000.
///
/// # Safety
/// `id` is a string id of the program.
unsafe fn string_bytes(id: u32) -> &'static [u8] {
    // SAFETY: the caller's contract; the length table matches the strings.
    unsafe {
        let length = *(&raw const CHIMI_STRING_LENGTHS).cast::<u32>().add(id as usize);
        core::slice::from_raw_parts(string(id).cast::<u8>(), length as usize)
    }
}

/// Whether the C strings `left` and `right` are equal.
///
/// # Safety
/// Both are NUL-terminated.
unsafe fn same_name(left: *const c_char, right: *const c_char) -> bool {
    // SAFETY: the caller's contract.
    unsafe { libc::strcmp(left, right) == 0 }
}

/// # Safety
/// `runtime` is the running program's.
#[inline(always)]
unsafe fn cancel_requested(runtime: *const Runtime) -> bool {
    // SAFETY: the caller's contract; a signal handler writes the flag.
    unsafe { ptr::read_volatile(&raw const (*runtime).cancel) != 0 }
}

/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn cancelled() -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { new_control(ControlKind::CANCEL, ptr::null_mut(), ptr::null(), -1) }
}

/// Runs the node `id` in `environment` under `parent`, unless the run was
/// cancelled.
///
/// # Safety
/// Runs on a fiber; `id` is a node of the program and `environment` live.
#[inline(always)]
pub unsafe fn run_node(id: u32, runtime: *mut Runtime, environment: *mut Environment, parent: u64) -> *mut Control {
    // SAFETY: the caller's contract; every node has a function.
    unsafe {
        if cancel_requested(runtime) {
            return cancelled();
        }
        let function = *(&raw const CHIMI_NODE_FUNCTIONS).cast::<NodeFn>().add(id as usize);
        function(runtime, environment, parent)
    }
}

/// `count` consecutive span ids that no other node has; returns the first.
/// One carrier takes them straight from the counter, so its ids stay dense
/// and in order, as a transcript expects; with several, ids are unique but
/// neither dense nor ordered across carriers.
///
/// # Safety
/// Runs on a carrier; `runtime` is the running program's.
pub unsafe fn take_spans(runtime: *mut Runtime, count: u64) -> u64 {
    // SAFETY: the caller's contract; the carrier is this thread's own.
    unsafe {
        let counter: &AtomicU64 = &(*runtime).next_span;
        if one_thread_runs_fibers() {
            return counter.fetch_add(count, Relaxed);
        }
        let here = current_carrier();
        if (*here).end_span.wrapping_sub((*here).next_span) < count {
            let size = count.max(SPAN_BLOCK);
            (*here).next_span = counter.fetch_add(size, Relaxed);
            (*here).end_span = (*here).next_span.wrapping_add(size);
        }
        let first = (*here).next_span;
        (*here).next_span = first.wrapping_add(count);
        first
    }
}

/// What a node's frame restores when it ends.
#[derive(Clone, Copy)]
pub struct Frame {
    /// The profiled node to charge again, plus one; 0 for none.
    outer: u32,
    span: u64,
}

/// Opens node `id`'s frame: charges the heap's allocations to its profile
/// row, if profiling, and takes its span and reports it started, if
/// tracing. A silent run's node takes its parent's span.
///
/// # Safety
/// Runs on a fiber; `id` is a node of the program; `operation` lives as
/// long as the program, since a profile row keeps it.
#[inline(always)]
pub unsafe fn begin(runtime: *mut Runtime, id: u32, parent: u64, operation: &CStr) -> Frame {
    // SAFETY: the caller's contract; a profile has a row per node.
    unsafe {
        let mut outer = 0;
        let rows = profile_rows();
        if !rows.is_null() {
            let row = rows.add(id as usize);
            AtomicPtr::from_ptr(ptr::addr_of_mut!((*row).operation).cast::<*mut c_char>())
                .store(operation.as_ptr().cast_mut(), Relaxed);
            AtomicU64::from_ptr(ptr::addr_of_mut!((*row).evaluations)).fetch_add(1, Relaxed);
            let heap = (*current_carrier()).fiber.heap;
            outer = (*heap).profiled_node_plus_one;
            (*heap).profiled_node_plus_one = id + 1;
        }
        if (*runtime).silent {
            return Frame { outer, span: parent };
        }
        let span = take_spans(runtime, 1);
        if (*runtime).full_trace {
            trace(runtime, c"operation_started", id, span, parent, operation, ptr::null());
        }
        Frame { outer, span }
    }
}

/// Closes node `id`'s frame with `result`, which it returns.
///
/// # Safety
/// As for [`begin`], with the frame it returned.
#[inline(always)]
pub unsafe fn finish(
    runtime: *mut Runtime,
    id: u32,
    parent: u64,
    frame: Frame,
    operation: &CStr,
    result: *mut Control,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if (*runtime).full_trace {
            trace(runtime, c"operation_completed", id, frame.span, parent, operation, result);
        }
        if !profile_rows().is_null() {
            (*(*current_carrier()).fiber.heap).profiled_node_plus_one = frame.outer;
        }
        result
    }
}

/// # Safety
/// As for `chimi_trace_event_v1`.
#[inline(never)]
unsafe fn trace(
    runtime: *mut Runtime,
    event: &CStr,
    id: u32,
    span: u64,
    parent: u64,
    operation: &CStr,
    outcome: *const Control,
) {
    // SAFETY: the caller's contract.
    unsafe { chimi_trace_event_v1(runtime, event.as_ptr(), id, span, parent as i64, operation.as_ptr(), outcome) }
}

/// One node's evaluation: its runtime, the environment it reads, and its
/// span, under which its children run.
#[derive(Clone, Copy)]
struct Eval {
    runtime: *mut Runtime,
    environment: *mut Environment,
    span: u64,
}

impl Eval {
    /// # Safety
    /// As for [`run_node`].
    #[inline(always)]
    unsafe fn run(self, child: u32) -> *mut Control {
        // SAFETY: the caller's contract.
        unsafe { run_node(child, self.runtime, self.environment, self.span) }
    }
}

/// Evaluates `count` children, whose node ids `child` gives, storing each
/// value with `store`; returns the first control that does not succeed.
///
/// # Safety
/// As for [`run_node`].
#[inline(always)]
unsafe fn each_child(
    eval: Eval,
    count: u32,
    child: impl Fn(u32) -> u32,
    mut store: impl FnMut(u32, *mut Value),
) -> Option<*mut Control> {
    // SAFETY: the caller's contract.
    unsafe {
        for index in 0..count {
            let control = eval.run(child(index));
            if (*control).kind != ControlKind::SUCCESS {
                return Some(control);
            }
            store(index, (*control).value);
        }
        None
    }
}

macro_rules! node_entry {
    ($($export:ident $operation:literal $body:ident;)*) => {$(
        #[doc = concat!("The `", $operation, "` node `node_id`, under `parent`.")]
        ///
        /// # Safety
        /// Runs on a fiber; `node_id` is such a node of the program and
        /// `incoming` its live environment.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $export(
            runtime: *mut Runtime,
            node_id: u32,
            incoming: *mut Environment,
            parent: u64,
        ) -> *mut Control {
            const OPERATION: &CStr = match CStr::from_bytes_with_nul(concat!($operation, "\0").as_bytes()) {
                Ok(name) => name,
                Err(_) => panic!(),
            };
            // SAFETY: the caller's contract.
            unsafe {
                let frame = begin(runtime, node_id, parent, OPERATION);
                let eval = Eval { runtime, environment: incoming, span: frame.span };
                let result = $body(eval, node_id);
                finish(runtime, node_id, parent, frame, OPERATION, result)
            }
        }
    )*};
}

node_entry! {
    chimi_rt_constant "constant" constant;
    chimi_rt_reference "reference" reference;
    chimi_rt_select "select" select;
    chimi_rt_dependency_method "dependency_method" dependency_method;
    chimi_rt_construct "construct" construct;
    chimi_rt_call "call" call;
    chimi_rt_list "list" list_node;
    chimi_rt_record "record" record;
    chimi_rt_tuple "tuple" tuple;
    chimi_rt_unary "unary" unary;
    chimi_rt_binary "binary" binary;
    chimi_rt_identity "identity" identity;
    chimi_rt_provision "provision" provision;
    chimi_rt_sequence "sequence" sequence;
    chimi_rt_closure "closure" closure;
    chimi_rt_match "match" match_node;
    chimi_rt_break "break" break_node;
}

/// # Safety
/// For each node body: runs on a fiber; `id` is a node of its operation.
#[inline(always)]
unsafe fn constant(_: Eval, id: u32) -> *mut Control {
    // SAFETY: the caller's contract; a literal's payload is as `a` says.
    unsafe {
        let node = node(id);
        let value = match node.a {
            Core::LITERAL_INTEGER => integer_value(ValueKind::INTEGER, node.integer),
            Core::LITERAL_FLOAT => float_value(f64::from_bits(node.integer as u64)),
            Core::LITERAL_TEXT => string_text(node.b),
            Core::LITERAL_VOID => void_value(),
            _ => boolean_value(node.integer != 0),
        };
        success(value)
    }
}

/// A Text from the string table, which may hold U+0000.
///
/// # Safety
/// Runs on a fiber that may allocate; `id` is a string id.
unsafe fn string_text(id: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let bytes = string_bytes(id);
        text_value(bytes.as_ptr().cast(), bytes.len())
    }
}

/// The declared function of the symbol `symbol_index`.
///
/// # Safety
/// The program's function table is live.
pub unsafe fn find_function(symbol_index: u32) -> *const Function {
    // SAFETY: the caller's contract; each index is below the count.
    unsafe {
        let table = (&raw const CHIMI_FUNCTIONS).cast::<Function>();
        (0..CHIMI_FUNCTION_COUNT)
            .map(|index| table.add(index as usize))
            .find(|&function| (*function).symbol == symbol_index)
            .unwrap_or(ptr::null())
    }
}

/// A new callable of `kind` named `name`; the caller fills in the rest.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn new_callable(kind: CallableKind, symbol_index: u32, name: *const c_char) -> *mut Callable {
    // SAFETY: the caller's contract.
    unsafe {
        let callable = allocate_kind::<Callable>(1, AllocationKind::CALLABLE);
        (*callable).kind = kind;
        (*callable).symbol = symbol_index;
        (*callable).name = name;
        callable
    }
}

/// # Safety
/// Runs on a fiber that may allocate; `callable` is live.
unsafe fn callable_value(callable: *mut Callable) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { pointer_value(ValueKind::CALLABLE, callable.cast::<c_void>()) }
}

/// The value the symbol a reference names has in `environment`.
///
/// # Safety
/// As for a node body.
unsafe fn reference(eval: Eval, id: u32) -> *mut Control {
    // SAFETY: the caller's contract; the compiler verified the reference.
    unsafe {
        let symbol_index = node(id).a;
        let shape = symbol(symbol_index);
        let environment = eval.environment;
        match shape.kind {
            Core::SYMBOL_FUNCTION => {
                let function = find_function(symbol_index);
                let callable = new_callable(CallableKind::DECLARED, symbol_index, ptr::null());
                (*callable).body = (*function).body;
                (*callable).parameters_offset = (*function).parameters_offset;
                (*callable).parameters_length = (*function).parameters_length;
                (*callable).local_count = (*function).local_count;
                (*callable).environment = crate::heap::values::capture_requirements(environment);
                (*callable).name = c"function".as_ptr();
                success(callable_value(callable))
            }
            Core::SYMBOL_CONSTANT => {
                let constants = chimi_runtime_constant_values.load(Relaxed);
                success(*constants.add(shape.constant as usize))
            }
            Core::SYMBOL_REQUIREMENT => {
                let provider = *(*environment).requirements.add(shape.environment_slot as usize);
                if !provider.is_null() {
                    return success(provider);
                }
                // The runner closes `clock: Clock` with the execution clock provider.
                if string_bytes(shape.name) == b"clock" {
                    return success(chimi_runtime_execution_clock_value_v1());
                }
                defect(c"closed core has no provider", -1)
            }
            Core::SYMBOL_LOCAL => {
                let local = *(*environment).locals.add(shape.environment_slot as usize);
                if local.is_null() { defect(c"verified local is not bound", -1) } else { success(local) }
            }
            Core::SYMBOL_CONSTRUCTOR => {
                success(callable_value(new_callable(CallableKind::CONSTRUCTOR, symbol_index, string(shape.name))))
            }
            Core::SYMBOL_INTRINSIC => {
                let callable = allocate_kind::<Callable>(1, AllocationKind::CALLABLE);
                (*callable).kind = CallableKind::INTRINSIC;
                (*callable).intrinsic = shape.intrinsic;
                (*callable).symbol = symbol_index;
                (*callable).name = string(shape.name);
                success(callable_value(callable))
            }
            Core::SYMBOL_PROCESS_NAME => {
                let value = new_value(ValueKind::PROCESS_ADDRESS);
                (*value).payload.address.kind = crate::heap::layout::Trace::ADDRESS_NAMED;
                (*value).payload.address.target = u64::from(symbol_index) + 1;
                success(value)
            }
            _ => defect(c"symbol is not a runtime value", -1),
        }
    }
}

/// The field `name` of a record or constructed value, or null.
///
/// # Safety
/// `value` is live; `name` is NUL-terminated.
pub unsafe fn select_value(value: *const Value, name: *const c_char) -> *mut Value {
    // SAFETY: the caller's contract; the kind says which payload is live,
    // and these fields are all named.
    unsafe {
        let (fields, length) = match (*value).kind {
            ValueKind::RECORD => ((*value).payload.record.fields, (*value).payload.record.length),
            ValueKind::CONSTRUCTED => ((*value).payload.constructed.fields, (*value).payload.constructed.length),
            _ => return ptr::null_mut(),
        };
        for index in 0..length as usize {
            let field = &*fields.add(index);
            if same_name(field.name, name) {
                return field.value;
            }
        }
        ptr::null_mut()
    }
}

/// # Safety
/// As for a node body.
unsafe fn select(eval: Eval, id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(id);
        let control = eval.run(node.a);
        if (*control).kind != ControlKind::SUCCESS {
            return control;
        }
        let value = select_value((*control).value, string(node.b));
        if value.is_null() { defect(c"verified value has no field", i64::from(id)) } else { success(value) }
    }
}

/// A method of a need, as a dependency callable: the bound provider's
/// method, or the runner's host function when none is bound.
///
/// # Safety
/// As for a node body.
unsafe fn dependency_method(eval: Eval, id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(id);
        let provider = *(*eval.environment).requirements.add(slot(node.a));
        let inner;
        if provider.is_null() {
            let host = match (symbol(node.b).standard_type, node.c) {
                (Core::TYPE_FILE_SYSTEM, 0) => Core::HOST_PROCESS_UTF8_LINES,
                (Core::TYPE_CONSOLE, 0) => Core::HOST_WRITE_LINE,
                (Core::TYPE_CONSOLE, 1) => Core::HOST_WRITE_ERROR,
                (Core::TYPE_RANDOM, 0) => Core::HOST_RANDOM_NEXT_INT,
                (Core::TYPE_CLOCK, 0) => Core::HOST_CLOCK_NOW,
                (Core::TYPE_CLOCK, 1) => Core::HOST_CLOCK_TIMESTAMP,
                (Core::TYPE_CLOCK, 2) => Core::HOST_CLOCK_SLEEP_UNTIL,
                _ => return defect(c"closed core has no provider", i64::from(id)),
            };
            inner = allocate_kind::<Callable>(1, AllocationKind::CALLABLE);
            (*inner).kind = CallableKind::HOST;
            (*inner).host = host;
        } else {
            let method = select_value(provider, string(node.d));
            if method.is_null() || (*method).kind != ValueKind::CALLABLE {
                return defect(c"provider has no callable field", i64::from(id));
            }
            let mut found = (*method).payload.callable;
            // A provider built from another need's methods (`let c = console`,
            // then `using { console: c }`) already holds dependency callables;
            // one call is one dependency call, of the need this node reads.
            if (*found).kind == CallableKind::DEPENDENCY {
                found = (*found).inner;
            }
            inner = found;
        }
        let callable = allocate_kind::<Callable>(1, AllocationKind::CALLABLE);
        (*callable).kind = CallableKind::DEPENDENCY;
        (*callable).inner = inner;
        (*callable).requirement = node.a;
        (*callable).interface_symbol = node.b;
        (*callable).method = node.c;
        (*callable).name = string(node.d);
        success(callable_value(callable))
    }
}

/// The constructor `symbol_index` applied to its fields' values: a
/// constructed value, or the success or failure an outcome constructor
/// makes.
///
/// # Safety
/// Runs on a fiber that may allocate; `values` holds a live value per field.
pub unsafe fn construct_value(symbol_index: u32, values: *const *mut Value) -> *mut Control {
    // SAFETY: the caller's contract; the program's tables are consistent.
    unsafe {
        let shape = entry(&raw const CHIMI_CONSTRUCTORS, symbol(symbol_index).constructor);
        if shape.outcome_success >= 0 {
            return if shape.outcome_success != 0 { success(*values) } else { failure(*values) };
        }
        let value = new_value(ValueKind::CONSTRUCTED);
        let constructed = &raw mut (*value).payload.constructed;
        (*constructed).constructor = symbol_index;
        (*constructed).type_name = string(shape.type_name);
        (*constructed).variant_name =
            if shape.variant_name == u32::MAX { ptr::null() } else { string(shape.variant_name) };
        (*constructed).length = shape.fields_length;
        let fields = allocate_kind::<Field>(shape.fields_length as usize, AllocationKind::FIELDS);
        (*constructed).fields = fields;
        for field in 0..shape.fields_length {
            *fields.add(field as usize) =
                Field { name: string(id(shape.fields_offset + field)), value: *values.add(field as usize) };
        }
        success(value)
    }
}

/// # Safety
/// As for a node body.
unsafe fn construct(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(node_id);
        let values = allocate_kind::<*mut Value>(node.length as usize, AllocationKind::BYTES);
        if let Some(stopped) =
            each_child(eval, node.length, |index| id(node.offset + index), |index, value| *values.add(index as usize) = value)
        {
            return stopped;
        }
        let result = construct_value(node.a, values);
        if (*result).kind == ControlKind::FAILURE {
            (*result).node = i64::from(node_id);
        }
        // A Failure(...) or Result.Err(...) in a cause arm: `b` is the arm's
        // cause local plus one, and `c` says whether it keeps that cause.
        if node.b == 0 {
            return result;
        }
        let mut matched = *(*eval.environment).locals.add(slot(node.b - 1));
        if !matched.is_null() && (*matched).kind != ValueKind::CAUSE {
            matched = ptr::null_mut();
        }
        if (*result).kind == ControlKind::FAILURE {
            let cause = cause_derived(matched, node.c != 0, (*result).value, i64::from(node_id));
            if !cause.is_null() {
                return control_with_cause(result, cause);
            }
        } else if (*result).kind == ControlKind::SUCCESS
            && (*(*result).value).kind == ValueKind::CONSTRUCTED
            && (*(*result).value).payload.constructed.length == 1
        {
            let constructed = (*(*result).value).payload.constructed;
            let cause = cause_derived(matched, node.c != 0, (*constructed.fields).value, i64::from(node_id));
            if !cause.is_null() {
                return success(with_stored_cause((*result).value, cause));
            }
        }
        result
    }
}

/// # Safety
/// As for a node body.
unsafe fn call(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(node_id);
        let callee = eval.run(node.a);
        if (*callee).kind != ControlKind::SUCCESS {
            return callee;
        }
        if (*(*callee).value).kind != ValueKind::CALLABLE {
            return defect(c"verified call target is not callable", i64::from(node_id));
        }
        let arguments = allocate_kind::<*mut Value>(node.length as usize, AllocationKind::BYTES);
        if let Some(stopped) = each_child(
            eval,
            node.length,
            |index| id(node.offset + index),
            |index, value| *arguments.add(index as usize) = value,
        ) {
            return stopped;
        }
        let result = invoke(
            eval.runtime,
            (*(*callee).value).payload.callable,
            arguments,
            node.length,
            eval.span,
            node_id,
            eval.environment,
        );
        // A failure an operation raised without a source construction starts at its call.
        if (*result).kind == ControlKind::FAILURE && (*result).cause.is_null() && (*result).node < 0 {
            return new_control(ControlKind::FAILURE, (*result).value, ptr::null(), i64::from(node_id));
        }
        result
    }
}

/// # Safety
/// As for a node body.
unsafe fn list_node(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(node_id);
        let items = new_value_slots(node.length);
        if let Some(stopped) =
            each_child(eval, node.length, |index| id(node.offset + index), |index, value| *items.add(index as usize) = value)
        {
            return stopped;
        }
        success(list::list_value(list::from_values(items, node.length), 0, node.length))
    }
}

/// A record or tuple of `length` fields, allocated before they are.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn new_fields_value(kind: ValueKind, length: u32) -> (*mut Value, *mut Field) {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(kind);
        (*value).payload.record.length = length;
        let fields = allocate_kind::<Field>(length as usize, AllocationKind::FIELDS);
        (*value).payload.record.fields = fields;
        (value, fields)
    }
}

/// # Safety
/// As for a node body.
unsafe fn record(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(node_id);
        let (value, fields) = new_fields_value(ValueKind::RECORD, node.length);
        let field = |index: u32| entry(&raw const CHIMI_NAMED_NODES, node.offset + index);
        if let Some(stopped) = each_child(
            eval,
            node.length,
            |index| field(index).node,
            |index, item| *fields.add(index as usize) = Field { name: string(field(index).name), value: item },
        ) {
            return stopped;
        }
        success(value)
    }
}

/// # Safety
/// As for a node body.
unsafe fn tuple(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(node_id);
        let (value, fields) = new_fields_value(ValueKind::TUPLE, node.length);
        if let Some(stopped) = each_child(
            eval,
            node.length,
            |index| id(node.offset + index),
            |index, item| *fields.add(index as usize) = Field { name: ptr::null(), value: item },
        ) {
            return stopped;
        }
        success(value)
    }
}

/// # Safety
/// As for a node body.
unsafe fn unary(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract; the kind says which payload is live.
    unsafe {
        let control = eval.run(node(node_id).a);
        if (*control).kind != ControlKind::SUCCESS {
            return control;
        }
        let value = (*control).value;
        match (*value).kind {
            ValueKind::FLOAT => success(float_value(-(*value).payload.floating)),
            ValueKind::DECIMAL => success(decimal_negated(value)),
            ValueKind::INTEGER if (*value).payload.integer == i64::MIN => numeric_defect(
                Message::new().text(b"integer overflow in unary `-`: -(").integer(i64::MIN).text(b")"),
                i64::from(node_id),
            ),
            ValueKind::INTEGER => success(integer_value(ValueKind::INTEGER, -(*value).payload.integer)),
            _ => defect(c"verified unary operand is not an integer", i64::from(node_id)),
        }
    }
}


/// A defect at `node` with a numeric message.
///
/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn numeric_defect(message: Message, node: i64) -> *mut Control {
    // SAFETY: the caller's contract; the message lives until the program ends.
    unsafe { defect_with(message.finish(), node) }
}

/// Kept out of line, as the arithmetic below is: the binary and unary nodes
/// sit in every frame of a recursion through an operator, and the decimals
/// and text buffers would grow each of those frames (the task stack test in
/// tests/cli.rs recurses 1,000 calls through `+`).
///
/// # Safety
/// Runs on a fiber that may allocate; `value` is a live Decimal.
#[inline(never)]
unsafe fn decimal_negated(value: *const Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { decimal_value(decimals::payload(decimals::of_payload((*value).payload.decimal).negate())) }
}

fn binary_symbol(operator: u32) -> &'static [u8] {
    match operator {
        Core::BINARY_ADD => b"+",
        Core::BINARY_SUBTRACT => b"-",
        Core::BINARY_MULTIPLY => b"*",
        Core::BINARY_DIVIDE => b"/",
        Core::BINARY_REMAINDER => b"%",
        _ => b"?",
    }
}

fn is_arithmetic(operator: u32) -> bool {
    matches!(
        operator,
        Core::BINARY_ADD | Core::BINARY_SUBTRACT | Core::BINARY_MULTIPLY | Core::BINARY_DIVIDE | Core::BINARY_REMAINDER
    )
}

/// The exact result, or `None` on overflow or a zero divisor.
#[inline(always)]
fn integer_result(operator: u32, left: i64, right: i64) -> Option<i64> {
    match operator {
        Core::BINARY_ADD => left.checked_add(right),
        Core::BINARY_SUBTRACT => left.checked_sub(right),
        Core::BINARY_MULTIPLY => left.checked_mul(right),
        Core::BINARY_DIVIDE => left.checked_div(right),
        // i64::MIN % -1 overflows in the machine instruction but is exactly 0.
        Core::BINARY_REMAINDER if right == -1 => Some(0),
        Core::BINARY_REMAINDER => left.checked_rem(right),
        _ => None,
    }
}

/// # Safety
/// Runs on a fiber that may allocate.
#[inline(always)]
unsafe fn integer_arithmetic(operator: u32, left: i64, right: i64, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        match integer_result(operator, left, right) {
            Some(output) => success(integer_value(ValueKind::INTEGER, output)),
            None => integer_defect(operator, left, right, node),
        }
    }
}

/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn integer_defect(operator: u32, left: i64, right: i64, node: i64) -> *mut Control {
    let symbol = binary_symbol(operator);
    let what: &[u8] = if right == 0 && matches!(operator, Core::BINARY_DIVIDE | Core::BINARY_REMAINDER) {
        b"integer division by zero in `"
    } else {
        b"integer overflow in `"
    };
    let message = Message::new()
        .text(what)
        .text(symbol)
        .text(b"`: ")
        .integer(left)
        .text(b" ")
        .text(symbol)
        .text(b" ")
        .integer(right);
    // SAFETY: the caller's contract.
    unsafe { numeric_defect(message, node) }
}

/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn float_arithmetic(operator: u32, left: f64, right: f64, node: i64) -> *mut Control {
    let symbol = binary_symbol(operator);
    let operands = |message: Message| {
        message
            .text(symbol)
            .text(b"`: ")
            .text(crate::float::text(left).as_bytes())
            .text(b" ")
            .text(symbol)
            .text(b" ")
            .text(crate::float::text(right).as_bytes())
    };
    let output = match operator {
        Core::BINARY_ADD => left + right,
        Core::BINARY_SUBTRACT => left - right,
        Core::BINARY_MULTIPLY => left * right,
        Core::BINARY_DIVIDE | Core::BINARY_REMAINDER => {
            if right == 0.0 {
                let message = operands(Message::new().text(b"float division by zero in `"));
                // SAFETY: the caller's contract.
                return unsafe { numeric_defect(message, node) };
            }
            // SAFETY: a pure C library function.
            if operator == Core::BINARY_DIVIDE { left / right } else { unsafe { fmod(left, right) } }
        }
        _ => 0.0,
    };
    // SAFETY: the caller's contract.
    unsafe {
        if !output.is_finite() {
            let message = Message::new()
                .text(b"float `")
                .text(symbol)
                .text(b"` has no finite result: ")
                .text(crate::float::text(left).as_bytes())
                .text(b" ")
                .text(symbol)
                .text(b" ")
                .text(crate::float::text(right).as_bytes());
            return numeric_defect(message, node);
        }
        success(float_value(output))
    }
}

/// Applies `+`, `-`, or `*`; typing rejects `/` and `%` on Decimal (T4306).
///
/// # Safety
/// Runs on a fiber that may allocate; both values are live Decimals.
#[inline(never)]
unsafe fn decimal_arithmetic(operator: u32, left: *const Value, right: *const Value, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let (l, r) = (decimals::of_payload((*left).payload.decimal), decimals::of_payload((*right).payload.decimal));
        let symbol = binary_symbol(operator);
        let output: Option<Decimal> = match operator {
            Core::BINARY_ADD => l.add(r),
            Core::BINARY_SUBTRACT => l.subtract(r),
            Core::BINARY_MULTIPLY => l.multiply(r),
            _ => {
                return numeric_defect(Message::new().text(b"`").text(symbol).text(b"` is not a `Decimal` operator"), node);
            }
        };
        match output {
            Some(output) => success(decimal_value(decimals::payload(output))),
            None => numeric_defect(
                Message::new()
                    .text(b"decimal overflow in `")
                    .text(symbol)
                    .text(b"`: ")
                    .text(l.text().as_bytes())
                    .text(b" ")
                    .text(symbol)
                    .text(b" ")
                    .text(r.text().as_bytes())
                    .text(b" needs more than 38 digits or scale 38"),
                node,
            ),
        }
    }
}

/// # Safety
/// As for a node body.
unsafe fn binary(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract; each kind says which payload is live.
    unsafe {
        let node = node(node_id);
        let left = eval.run(node.b);
        if (*left).kind != ControlKind::SUCCESS {
            return left;
        }
        let right = eval.run(node.c);
        if (*right).kind != ControlKind::SUCCESS {
            return right;
        }
        let (left, right) = ((*left).value, (*right).value);
        let at = i64::from(node_id);
        if is_arithmetic(node.a) {
            return match ((*left).kind, (*right).kind) {
                (ValueKind::INTEGER, ValueKind::INTEGER) => {
                    integer_arithmetic(node.a, (*left).payload.integer, (*right).payload.integer, at)
                }
                (ValueKind::FLOAT, ValueKind::FLOAT) => {
                    float_arithmetic(node.a, (*left).payload.floating, (*right).payload.floating, at)
                }
                (ValueKind::DECIMAL, ValueKind::DECIMAL) => decimal_arithmetic(node.a, left, right, at),
                _ => defect(c"verified binary operands are not numbers", at),
            };
        }
        if node.a == Core::BINARY_RANGE || node.a == Core::BINARY_RANGE_INCLUSIVE {
            if (*left).kind != ValueKind::INTEGER || (*right).kind != ValueKind::INTEGER {
                return defect(c"verified range bounds are not integers", at);
            }
            let mut end = (*right).payload.integer;
            if node.a == Core::BINARY_RANGE_INCLUSIVE {
                if end == i64::MAX {
                    return defect(
                        c"the inclusive range `..=` ends at the largest Int, so its exclusive end overflows",
                        at,
                    );
                }
                end += 1;
            }
            return success(range_value((*left).payload.integer, end));
        }
        let order = compare(left, right);
        let holds = match node.a {
            Core::BINARY_EQUAL => order == 0,
            Core::BINARY_LESS => order < 0,
            Core::BINARY_LESS_EQUAL => order <= 0,
            Core::BINARY_GREATER => order > 0,
            Core::BINARY_GREATER_EQUAL => order >= 0,
            _ => return defect(c"verified binary operator is unknown", at),
        };
        success(boolean_value(holds))
    }
}

/// # Safety
/// As for a node body.
unsafe fn identity(eval: Eval, id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { eval.run(node(id).a) }
}

/// Binds `value` to every requirement named `name` in `environment`, and
/// to the clock slot when the name is `clock`, as a provision does.
///
/// # Safety
/// `environment`'s requirement table is its own; `name` is NUL-terminated.
pub unsafe fn bind_requirement(environment: *mut Environment, name: *const c_char, value: *mut Value) {
    // SAFETY: the caller's contract; each index is below the count.
    unsafe {
        let requirements = (*environment).requirements;
        for index in 0..CHIMI_SYMBOL_COUNT {
            let shape = symbol(index);
            if shape.kind == Core::SYMBOL_REQUIREMENT && same_name(string(shape.name), name) {
                *requirements.add(shape.environment_slot as usize) = value;
            }
        }
        if CStr::from_ptr(name).to_bytes() == b"clock" {
            *requirements.add(CHIMI_CLOCK_REQUIREMENT_SLOT as usize) = value;
        }
    }
}

/// # Safety
/// As for a node body.
unsafe fn provision(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(node_id);
        let environment = clone_environment(eval.environment, false, true);
        for index in 0..node.length {
            // Each provider runs in the enclosing scope, as typing gives its
            // own needs to the enclosing expression: no provider sees another
            // of this clause.
            let provider = entry(&raw const CHIMI_NAMED_NODES, node.offset + index);
            let control = eval.run(provider.node);
            if (*control).kind != ControlKind::SUCCESS {
                return control;
            }
            bind_requirement(environment, string(provider.name), (*control).value);
        }
        Eval { environment, ..eval }.run(node.a)
    }
}

/// The locals are cloned at the first binding step, so a block that binds
/// nothing (a loop body such as `{ case ... }`) allocates no environment.
/// Steps before that one read the incoming locals, which the clone would
/// have copied unchanged.
///
/// # Safety
/// As for a node body.
unsafe fn sequence(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(node_id);
        let mut eval = eval;
        let incoming = eval.environment;
        let mut last = ptr::null_mut();
        for index in 0..node.length {
            let step = entry(&raw const CHIMI_STEPS, node.offset + index);
            let control = eval.run(step.node);
            if (*control).kind != ControlKind::SUCCESS {
                return control;
            }
            last = (*control).value;
            if step.kind == 0 {
                if eval.environment == incoming {
                    eval.environment = clone_environment(incoming, true, false);
                }
                *(*eval.environment).locals.add(slot(step.symbol)) = last;
            }
        }
        success(last)
    }
}

/// # Safety
/// As for a node body.
unsafe fn closure(eval: Eval, id_: u32) -> *mut Control {
    // SAFETY: the caller's contract; `b` and `c` list the captured symbols.
    unsafe {
        let node = node(id_);
        let callable = allocate_kind::<Callable>(1, AllocationKind::CALLABLE);
        (*callable).kind = CallableKind::CLOSURE;
        (*callable).body = node.a;
        (*callable).parameters_offset = node.offset;
        (*callable).parameters_length = node.length;
        let source = eval.environment;
        let captured = environment(AllocationKind::ENVIRONMENT, node.d, ptr::null_mut(), ptr::null_mut());
        (*captured).locals = new_value_slots(node.d);
        (*captured).requirements = clone_value_slots((*source).requirements, requirement_slots());
        for index in 0..node.c {
            let local = slot(id(node.b + index));
            *(*captured).locals.add(local) = *(*source).locals.add(local);
        }
        (*callable).environment = captured;
        (*callable).name = c"closure".as_ptr();
        success(callable_value(callable))
    }
}

/// # Safety
/// As for a node body.
unsafe fn break_node(eval: Eval, id_: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(id_);
        let mut value = void_value();
        if node.b != u32::MAX {
            let control = eval.run(node.b);
            if (*control).kind != ControlKind::SUCCESS {
                return control;
            }
            value = (*control).value;
        }
        new_control(ControlKind::BREAK, value, ptr::null(), i64::from(node.a))
    }
}

/// What a match subject produced, as its arms see it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Plain,
    Success,
    Failure,
}

/// One subject's value; `control` is its outcome, which holds a failure's cause.
#[derive(Clone, Copy)]
struct MatchValue {
    kind: MatchKind,
    value: *mut Value,
    control: *mut Control,
}

/// The binding pass of a match. `view_failed` records a list view that could
/// not be taken, which the match reports as a defect.
struct PatternBinder {
    environment: *mut Environment,
    view_failed: bool,
}

/// # Safety
/// `id` is a pattern of the program.
#[inline(always)]
unsafe fn pattern(id: u32) -> &'static Pattern {
    // SAFETY: the caller's contract.
    unsafe { entry(&raw const CHIMI_PATTERNS, id) }
}

/// # Safety
/// `value` is live.
unsafe fn literal_matches(pattern: &Pattern, value: *const Value) -> bool {
    // SAFETY: the caller's contract; the kind says which payload is live.
    unsafe {
        match pattern.a {
            Core::LITERAL_INTEGER => (*value).kind == ValueKind::INTEGER && (*value).payload.integer == pattern.integer,
            Core::LITERAL_TEXT => {
                if (*value).kind != ValueKind::TEXT {
                    return false;
                }
                let text = (*value).payload.text;
                let expected = string_bytes(pattern.offset);
                text.length == expected.len()
                    && core::slice::from_raw_parts(text.bytes.cast::<u8>(), text.length) == expected
            }
            _ => (*value).kind == ValueKind::BOOLEAN && (*value).payload.boolean == (pattern.integer != 0),
        }
    }
}

/// Checks the length first, then reads each fixed element by index. In the
/// binding pass a named rest receives the view between the prefix and the
/// suffix.
///
/// # Safety
/// Runs on a fiber that may allocate; `value` is live.
unsafe fn match_list(pattern: &Pattern, value: *mut Value, binder: Option<&mut PatternBinder>) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        if (*value).kind != ValueKind::LIST {
            return false;
        }
        let length = (*value).payload.list.length;
        let prefix = pattern.a;
        let has_rest = u32::from(pattern.integer != 0);
        let suffix = pattern.length - prefix - has_rest;
        if if has_rest != 0 { length < prefix + suffix } else { length != prefix } {
            return false;
        }
        let suffix_start = length - suffix;
        let mut binder = binder;
        for index in 0..prefix {
            if !match_plain(id(pattern.offset + index), list::get(value, index), binder.as_deref_mut()) {
                return false;
            }
        }
        for index in 0..suffix {
            let element = list::get(value, suffix_start + index);
            if !match_plain(id(pattern.offset + prefix + has_rest + index), element, binder.as_deref_mut()) {
                return false;
            }
        }
        let Some(binder) = binder else { return true };
        if has_rest == 0 {
            return true;
        }
        let rest = id(pattern.offset + prefix);
        if self::pattern(rest).kind == Core::PATTERN_WILDCARD {
            return true;
        }
        let view = list::slice(current_carrier(), value, prefix, suffix_start);
        if view.is_null() {
            binder.view_failed = true;
            return false;
        }
        match_plain(rest, view, Some(binder))
    }
}

/// Whether `value` matches the pattern `pattern_id`, binding its names when
/// `binder` is given.
///
/// # Safety
/// Runs on a fiber that may allocate; `value` is live.
unsafe fn match_plain(pattern_id: u32, value: *mut Value, binder: Option<&mut PatternBinder>) -> bool {
    // SAFETY: the caller's contract; each kind says which payload is live.
    unsafe {
        let pattern = pattern(pattern_id);
        let mut binder = binder;
        let fields = |payload_fields: *mut Field, binder: Option<&mut PatternBinder>| {
            let mut binder = binder;
            (0..pattern.length).all(|index| {
                match_plain(id(pattern.offset + index), (*payload_fields.add(index as usize)).value, binder.as_deref_mut())
            })
        };
        match pattern.kind {
            Core::PATTERN_WILDCARD => true,
            Core::PATTERN_BINDING => {
                if let Some(binder) = binder {
                    *(*binder.environment).locals.add(slot(pattern.a)) = value;
                }
                true
            }
            Core::PATTERN_LITERAL => literal_matches(pattern, value),
            Core::PATTERN_CONSTRUCTOR => {
                let constructed = (*value).payload.constructed;
                (*value).kind == ValueKind::CONSTRUCTED
                    && constructed.constructor == pattern.a
                    && constructed.length == pattern.length
                    && fields(constructed.fields, binder)
            }
            Core::PATTERN_LIST => match_list(pattern, value, binder),
            Core::PATTERN_TUPLE => {
                let record = (*value).payload.record;
                (*value).kind == ValueKind::TUPLE && record.length == pattern.length && fields(record.fields, binder)
            }
            Core::PATTERN_ALIAS => {
                if !match_plain(pattern.offset, value, binder.as_deref_mut()) {
                    return false;
                }
                if let Some(binder) = binder {
                    *(*binder.environment).locals.add(slot(pattern.a)) = value;
                }
                true
            }
            _ => false,
        }
    }
}

/// # Safety
/// As for [`match_plain`].
unsafe fn match_value(pattern_id: u32, value: MatchValue, binder: Option<&mut PatternBinder>) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        let pattern = pattern(pattern_id);
        if pattern.kind == Core::PATTERN_OUTCOME {
            let expected = if pattern.a != 0 { MatchKind::Success } else { MatchKind::Failure };
            return value.kind == expected && match_plain(pattern.offset, value.value, binder);
        }
        value.kind == MatchKind::Plain && match_plain(pattern_id, value.value, binder)
    }
}

/// Whether the pattern `pattern_id` binds a name.
///
/// # Safety
/// `pattern_id` is a pattern of the program.
unsafe fn pattern_binds(pattern_id: u32) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        let pattern = pattern(pattern_id);
        match pattern.kind {
            Core::PATTERN_BINDING | Core::PATTERN_ALIAS => true,
            Core::PATTERN_OUTCOME => pattern_binds(pattern.offset),
            Core::PATTERN_CONSTRUCTOR | Core::PATTERN_LIST | Core::PATTERN_TUPLE => {
                (0..pattern.length).any(|index| pattern_binds(id(pattern.offset + index)))
            }
            _ => false,
        }
    }
}

/// # Safety
/// As for a node body.
unsafe fn match_node(eval: Eval, node_id: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = node(node_id);
        let at = i64::from(node_id);
        let arm = |index: u32| entry(&raw const CHIMI_ARMS, node.a + index);
        let subjects = node.length as usize;
        let mut stack_values = [MatchValue { kind: MatchKind::Plain, value: ptr::null_mut(), control: ptr::null_mut() };
            MATCH_STACK_SUBJECTS];
        let values: *mut MatchValue = if subjects <= MATCH_STACK_SUBJECTS {
            stack_values.as_mut_ptr()
        } else {
            allocate_kind::<MatchValue>(subjects, AllocationKind::BYTES)
        };
        for subject in 0..node.length {
            let contextual = (0..node.b)
                .any(|index| pattern(id(arm(index).patterns_offset + subject)).kind == Core::PATTERN_OUTCOME);
            let control = eval.run(id(node.offset + subject));
            let kind = match ((*control).kind, contextual) {
                (ControlKind::SUCCESS, true) => MatchKind::Success,
                (ControlKind::FAILURE, true) => MatchKind::Failure,
                (ControlKind::SUCCESS, false) => MatchKind::Plain,
                _ => return control,
            };
            *values.add(subject as usize) = MatchValue { kind, value: (*control).value, control };
        }
        let value = |subject: u32| *values.add(subject as usize);
        for index in 0..node.b {
            let arm = arm(index);
            let pattern_of = |subject: u32| id(arm.patterns_offset + subject);
            if !(0..node.length).all(|subject| match_value(pattern_of(subject), value(subject), None)) {
                continue;
            }
            let binds =
                arm.cause_local != u32::MAX || (0..node.length).any(|subject| pattern_binds(pattern_of(subject)));
            let arm_environment = if binds { clone_environment(eval.environment, true, false) } else { eval.environment };
            let mut binder = PatternBinder { environment: arm_environment, view_failed: false };
            if binds {
                for subject in 0..node.length {
                    if !match_value(pattern_of(subject), value(subject), Some(&mut binder)) {
                        return if binder.view_failed {
                            defect(c"verified list pattern took a view outside its list", at)
                        } else {
                            defect(c"verified pattern changed while binding", at)
                        };
                    }
                }
            }
            let arm_eval = Eval { environment: arm_environment, ..eval };
            if arm.guard != u32::MAX {
                let guard = arm_eval.run(arm.guard);
                if (*guard).kind != ControlKind::SUCCESS {
                    return guard;
                }
                if (*(*guard).value).kind != ValueKind::BOOLEAN {
                    return defect(c"verified case guard is not Boolean", at);
                }
                if !(*(*guard).value).payload.boolean {
                    continue;
                }
            }
            if arm.cause_local != u32::MAX {
                *(*arm_environment).locals.add(slot(arm.cause_local)) =
                    matched_cause_value(value(arm.cause_subject).control);
            }
            return arm_eval.run(arm.body);
        }
        for subject in 0..node.length {
            if value(subject).kind == MatchKind::Failure {
                return value(subject).control;
            }
        }
        defect(c"verified exhaustive match had no matching arm", at)
    }
}

/// Whether the host stack has reached its guard, measured from this frame.
///
/// # Safety
/// Runs on a carrier.
#[inline(always)]
unsafe fn host_stack_exhausted() -> bool {
    let marker = 0u8;
    let here = core::hint::black_box(&marker) as *const u8 as usize;
    // SAFETY: the caller's contract.
    unsafe { here < (*current_carrier()).host_stack_limit }
}

/// Calls `callable` with `argument_count` arguments, under `span`, for the
/// call node `call_node`, whose environment's providers the callee sees.
///
/// # Safety
/// Runs on a fiber; `callable`, the arguments and `call_environment` are live.
pub unsafe fn invoke(
    runtime: *mut Runtime,
    callable: *mut Callable,
    arguments: *mut *mut Value,
    argument_count: u32,
    span: u64,
    call_node: u32,
    call_environment: *mut Environment,
) -> *mut Control {
    // SAFETY: the caller's contract; a callable's kind says which fields it set.
    unsafe {
        let mut callable = callable;
        loop {
            match (*callable).kind {
                CallableKind::CONSTRUCTOR => return construct_value((*callable).symbol, arguments),
                CallableKind::DEPENDENCY => {
                    if !(*runtime).silent {
                        let dependency_span = take_spans(runtime, 1);
                        chimi_trace_dependency_v1(runtime, call_node, dependency_span, span, callable);
                    }
                    callable = (*callable).inner;
                }
                CallableKind::INTRINSIC | CallableKind::HOST => {
                    return chimi_runtime_invoke_native_v1(
                        runtime,
                        callable,
                        arguments,
                        argument_count,
                        span,
                        call_node,
                        call_environment,
                    );
                }
                _ => break,
            }
        }
        if host_stack_exhausted() {
            return defect(c"Stack overflow", -1);
        }
        let requirements = requirement_slots() as usize;
        let environment = if (*callable).kind == CallableKind::DECLARED {
            // Declared functions capture provisioned requirements, but never
            // local bindings: a clean local table for the parameters, with
            // the requirements taken when the function became a value.
            let frame = new_environment((*callable).local_count);
            ptr::copy_nonoverlapping((*(*callable).environment).requirements, (*frame).requirements, requirements);
            frame
        } else {
            clone_environment((*callable).environment, true, true)
        };
        for index in 0..requirements {
            let provided = *(*call_environment).requirements.add(index);
            if !provided.is_null() {
                *(*environment).requirements.add(index) = provided;
            }
        }
        let count = (*callable).parameters_length.min(argument_count);
        for index in 0..count {
            let parameter = slot(id((*callable).parameters_offset + index));
            *(*environment).locals.add(parameter) = *arguments.add(index as usize);
        }
        run_node((*callable).body, runtime, environment, span)
    }
}

/// [`invoke`], for `runtime.c` and the intrinsics that call back.
///
/// # Safety
/// As for [`invoke`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_invoke_v1(
    runtime: *mut Runtime,
    callable: *mut Callable,
    arguments: *mut *mut Value,
    argument_count: u32,
    span: u64,
    call_node: u32,
    environment: *mut Environment,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { invoke(runtime, callable, arguments, argument_count, span, call_node, environment) }
}

/// [`construct_value`].
///
/// # Safety
/// As for [`construct_value`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_construct_value_v1(symbol_index: u32, values: *const *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { construct_value(symbol_index, values) }
}

/// [`select_value`].
///
/// # Safety
/// As for [`select_value`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_select_value_v1(value: *const Value, name: *const c_char) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { select_value(value, name) }
}

/// [`find_function`].
///
/// # Safety
/// As for [`find_function`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_find_function_v1(symbol_index: u32) -> *const Function {
    // SAFETY: the caller's contract.
    unsafe { find_function(symbol_index) }
}

/// [`bind_requirement`].
///
/// # Safety
/// As for [`bind_requirement`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_bind_requirement_v1(
    environment: *mut Environment,
    name: *const c_char,
    value: *mut Value,
) {
    // SAFETY: the caller's contract.
    unsafe { bind_requirement(environment, name, value) }
}

/// [`take_spans`].
///
/// # Safety
/// As for [`take_spans`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_take_spans_v1(runtime: *mut Runtime, count: u64) -> u64 {
    // SAFETY: the caller's contract.
    unsafe { take_spans(runtime, count) }
}

/// [`begin`] for a node still in `runtime.c`; writes the frame's profile
/// state to `outer` and returns its span.
///
/// # Safety
/// As for [`begin`]; `operation` lives as long as the program, and `outer`
/// is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_node_begin_v1(
    runtime: *mut Runtime,
    id: u32,
    parent: u64,
    operation: *const c_char,
    outer: *mut u32,
) -> u64 {
    // SAFETY: the caller's contract.
    unsafe {
        let frame = begin(runtime, id, parent, CStr::from_ptr(operation));
        *outer = frame.outer;
        frame.span
    }
}

/// [`finish`] for a node still in `runtime.c`.
///
/// # Safety
/// As for [`finish`], with what [`chimi_runtime_node_begin_v1`] gave.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_node_finish_v1(
    runtime: *mut Runtime,
    id: u32,
    parent: u64,
    span: u64,
    operation: *const c_char,
    outer: u32,
    result: *mut Control,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { finish(runtime, id, parent, Frame { outer, span }, CStr::from_ptr(operation), result) }
}
