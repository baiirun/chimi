//! The runtime's telemetry events (CHI-305 step 5, phase 7): the JSON lines
//! `--trace` writes for computations, operations, dependency calls, process
//! bodies and the scheduler's own events, byte for byte as `runtime.c` wrote
//! them. One lock covers a whole event, so events from several carriers come
//! out whole and numbered in the order written; the scheduler's events
//! arrive from its trace hook under the kernel lock, so this lock is taken
//! after the kernel's.
//!
//! Numbers are written by hand, as the `%` conversions wrote them, so no
//! formatting machinery joins every program.

use core::ffi::{CStr, c_char, c_void};
use core::ptr;

use crate::heap::layout::{Callable, Control, ControlKind, Node, Origin, Runtime, SchedulerTraceEvent, Trace};
use crate::intrinsics::standard::{string, symbol};
use crate::pthread_locked::PthreadLocked;

unsafe extern "C" {
    static CHIMI_NODES: [Node; 0];
    static CHIMI_ORIGINS: [Origin; 0];
    /// The trace's module table, the fragment `computation_started` ends with.
    static CHIMI_TRACE_MODULES: *const c_char;
    /// The execution clock, in nanoseconds since its origin.
    fn chimi_fiber_now() -> i64;
    /// The name a process name symbol (its index plus one) spells, or null.
    fn chimi_runtime_process_name_label_v1(name: u64) -> *const c_char;
}

/// Held while one event is written.
static TRACE_LOCK: PthreadLocked<()> = PthreadLocked::new(());

/// Runs `body`, which writes one event, under the trace lock.
fn locked(body: impl FnOnce()) {
    TRACE_LOCK.with(|()| body())
}

/// Bytes to a C stream.
pub(crate) struct Out(pub(crate) *mut libc::FILE);

impl Out {
    pub(crate) fn bytes(&self, bytes: &[u8]) {
        // SAFETY: the stream is an open report file the runtime writes.
        unsafe { libc::fwrite(bytes.as_ptr().cast(), 1, bytes.len(), self.0) };
    }

    pub(crate) fn unsigned(&self, value: u64) {
        let mut digits = [0; 20];
        self.bytes(unsigned_spelling(value, &mut digits));
    }

    pub(crate) fn signed(&self, value: i64) {
        if value < 0 {
            self.bytes(b"-");
        }
        self.unsigned(value.unsigned_abs());
    }

    /// `bytes` as a JSON string, escaping as `runtime.c`'s `json_bytes` did.
    pub(crate) fn json(&self, bytes: &[u8]) {
        self.bytes(b"\"");
        let mut start = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            let escape: &[u8] = match byte {
                b'"' => b"\\\"",
                b'\\' => b"\\\\",
                b'\n' => b"\\n",
                b'\r' => b"\\r",
                b'\t' => b"\\t",
                0..0x20 => b"",
                _ => continue,
            };
            self.bytes(&bytes[start..index]);
            start = index + 1;
            if escape.is_empty() {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                self.bytes(&[b'\\', b'u', b'0', b'0', HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]]);
            } else {
                self.bytes(escape);
            }
        }
        self.bytes(&bytes[start..]);
        self.bytes(b"\"");
    }

    /// The C string `text` as a JSON string.
    ///
    /// # Safety
    /// `text` is a C string.
    pub(crate) unsafe fn json_c(&self, text: *const c_char) {
        // SAFETY: the caller's contract.
        self.json(unsafe { CStr::from_ptr(text) }.to_bytes());
    }

    /// `,"key":` and the identity, or null for 0.
    fn identity(&self, key: &[u8], identity: u64) {
        self.key(key);
        if identity == 0 { self.bytes(b"null") } else { self.unsigned(identity) }
    }

    /// `,"key":` and the word as a JSON string, or null.
    ///
    /// # Safety
    /// `word` is null or a C string.
    unsafe fn word(&self, key: &[u8], word: *const c_char) {
        self.key(key);
        // SAFETY: the caller's contract.
        if word.is_null() { self.bytes(b"null") } else { unsafe { self.json_c(word) } }
    }

    fn key(&self, key: &[u8]) {
        self.bytes(b",\"");
        self.bytes(key);
        self.bytes(b"\":");
    }

    /// The fields every event opens with: its name, sequence number,
    /// computation and execution.
    ///
    /// # Safety
    /// `runtime` is the running runtime, with the trace lock held.
    unsafe fn head(&self, runtime: *mut Runtime, event: &[u8]) {
        // SAFETY: the caller's contract; the lock guards the counters.
        unsafe {
            (*runtime).trace_events_constructed += 1;
            self.bytes(b"{\"schema\":\"chimi.runtime.v1\",\"event\":");
            self.json(event);
            self.bytes(b",\"sequence\":");
            self.unsigned((*runtime).next_sequence);
            (*runtime).next_sequence += 1;
            self.bytes(b",\"computation\":");
            self.json_c((*runtime).computation);
            self.bytes(b",\"execution_id\":");
            self.json_c((*runtime).execution_id);
        }
    }

    /// A node's identities: `core_node_id` through `source_end`.
    ///
    /// # Safety
    /// `node_id` is a node of the program.
    unsafe fn node(&self, node_id: u32) {
        // SAFETY: the caller's contract; every node names an origin.
        let (node, origin) = unsafe {
            let node = &*(&raw const CHIMI_NODES).cast::<Node>().add(node_id as usize);
            (node, &*(&raw const CHIMI_ORIGINS).cast::<Origin>().add(node.origin as usize))
        };
        for (key, value) in [
            (&b"core_node_id"[..], node_id),
            (b"origin_id", node.origin),
            (b"semantic_id", origin.semantic_id),
            (b"file_id", origin.file_id),
            (b"source_start", origin.start),
            (b"source_end", origin.end),
        ] {
            self.key(key);
            self.unsigned(u64::from(value));
        }
    }
}

/// The decimal digits of `value`, as `%PRIu64` writes them.
pub(crate) fn unsigned_spelling(mut value: u64, output: &mut [u8; 20]) -> &[u8] {
    let mut start = output.len();
    loop {
        start -= 1;
        output[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &output[start..];
        }
    }
}

/// The word an outcome is in telemetry.
fn control_name(kind: ControlKind) -> &'static [u8] {
    match kind {
        ControlKind::SUCCESS => b"success",
        ControlKind::FAILURE => b"expected_failure",
        ControlKind::CANCEL => b"cancellation",
        ControlKind::BREAK => b"break",
        _ => b"defect",
    }
}

/// `event` for `node_id`'s span: `computation_started` and
/// `computation_completed`, and `operation_started` and
/// `operation_completed` under `--trace=full`. A negative `parent` is none;
/// `outcome`, when not null, is the span's result.
///
/// # Safety
/// `runtime` is the running runtime; `event` and `operation` are C strings;
/// `node_id` is a node of the program; `outcome` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_trace_event_v1(
    runtime: *mut Runtime,
    event: *const c_char,
    node_id: u32,
    span: u64,
    parent: i64,
    operation: *const c_char,
    outcome: *const Control,
) {
    // SAFETY: the caller's contract.
    unsafe {
        let out = Out((*runtime).telemetry.cast());
        let event = CStr::from_ptr(event).to_bytes();
        locked(|| {
            out.head(runtime, event);
            out.bytes(b",\"span_id\":");
            out.unsigned(span);
            out.bytes(b",\"parent_span_id\":");
            if parent < 0 { out.bytes(b"null") } else { out.signed(parent) }
            out.node(node_id);
            out.bytes(b",\"operation\":");
            out.json_c(operation);
            out.bytes(b",\"attributes\":{");
            if !outcome.is_null() {
                out.bytes(b"\"outcome\":");
                out.json(control_name((*outcome).kind));
                out.bytes(b",");
            }
            // The computation starts at the clock's origin, before the scheduler runs.
            let started = event == b"computation_started";
            out.bytes(b"\"time_ns\":");
            out.signed(if started { 0 } else { chimi_fiber_now() });
            if started {
                out.bytes(b",\"start_unix_ns\":");
                out.signed((*runtime).start_unix_ns);
                out.bytes(CStr::from_ptr(CHIMI_TRACE_MODULES).to_bytes());
            }
            out.bytes(b"}}\n");
        });
    }
}

/// `dependency_called`: the call at `node_id` of a requirement's method.
///
/// # Safety
/// `runtime` is the running runtime; `node_id` is a node of the program;
/// `callable` is a live dependency method.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_trace_dependency_v1(
    runtime: *mut Runtime,
    node_id: u32,
    span: u64,
    parent: u64,
    callable: *const Callable,
) {
    // SAFETY: the caller's contract; a requirement is a symbol of the program.
    unsafe {
        let out = Out((*runtime).telemetry.cast());
        let callable = &*callable;
        locked(|| {
            out.head(runtime, b"dependency_called");
            out.bytes(b",\"span_id\":");
            out.unsigned(span);
            out.bytes(b",\"parent_span_id\":");
            out.unsigned(parent);
            out.node(node_id);
            out.bytes(b",\"operation\":\"dependency_call\",\"attributes\":{\"requirement\":");
            out.json_c(string(symbol(callable.requirement).name));
            out.bytes(b",\"requirement_symbol_id\":");
            out.unsigned(u64::from(callable.requirement));
            out.bytes(b",\"interface_symbol_id\":");
            out.unsigned(u64::from(callable.interface_symbol));
            out.bytes(b",\"method\":");
            out.json_c(callable.name);
            out.bytes(b",\"method_semantic_id\":");
            out.unsigned(u64::from(callable.method));
            out.bytes(b",\"time_ns\":");
            out.signed(chimi_fiber_now());
            out.bytes(b"}}\n");
        });
    }
}

/// A process body's own span (CHI-312): `process_body_started` when the
/// body begins, and `process_body_completed` with its outcome (not null)
/// when it ends, both naming the incarnation.
///
/// # Safety
/// `runtime` is the running runtime; `outcome` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_trace_process_body_v1(
    runtime: *mut Runtime,
    span: u64,
    incarnation: u64,
    outcome: *const Control,
) {
    // SAFETY: the caller's contract.
    unsafe {
        let out = Out((*runtime).telemetry.cast());
        locked(|| {
            out.head(runtime, if outcome.is_null() { b"process_body_started" } else { b"process_body_completed" });
            out.bytes(b",\"span_id\":");
            out.unsigned(span);
            out.bytes(b",\"parent_span_id\":null");
            out.identity(b"incarnation_id", incarnation);
            out.bytes(b",\"operation\":\"process_body\",\"attributes\":{");
            if !outcome.is_null() {
                out.bytes(b"\"outcome\":");
                out.json(control_name((*outcome).kind));
                out.bytes(b",");
            }
            out.bytes(b"\"time_ns\":");
            out.signed(chimi_fiber_now());
            out.bytes(b"}}\n");
        });
    }
}

/// The event name of a scheduler trace event, by family and kind.
fn scheduler_event_name(event: &SchedulerTraceEvent) -> &'static [u8] {
    const TASK: &[&[u8]] = &[
        b"task_started", b"task_parked", b"task_woken", b"task_cancellation_requested",
        b"task_cleanup_completed", b"task_terminal",
    ];
    const CHANNEL: &[&[u8]] = &[
        b"channel_created", b"channel_send_accepted", b"channel_send_parked",
        b"channel_send_rejected_closed", b"channel_send_rejected_aborted",
        b"channel_send_cancelled", b"channel_receive_delivered", b"channel_receive_parked",
        b"channel_receive_cancelled", b"channel_closed", b"channel_drained",
        b"channel_aborted", b"channel_receiver_transferred", b"channel_completion_transferred",
        b"channel_released",
    ];
    const CLOCK: &[&[u8]] = &[
        b"clock_advanced", b"clock_timer_started", b"clock_timer_fired",
        b"clock_timer_cancelled", b"clock_deadlock_waiter", b"clock_scope_fired",
        b"clock_wait_parked", b"clock_wait_committed", b"clock_signal_received",
    ];
    // The transcript table's names, in trace-kind order.
    const PROCESS: &[&[u8]] = &[
        b"process_started", b"admission_rejected", b"process_terminated", b"restart_attempted",
        b"restart_limit_exceeded", b"supervisor_escalated", b"membership_removed",
        b"supervisor_stopped", b"process_send", b"supervisor_started",
    ];
    let names = match event.family {
        Trace::FAMILY_TASK => TASK,
        Trace::FAMILY_CHANNEL => CHANNEL,
        Trace::FAMILY_CLOCK => CLOCK,
        Trace::FAMILY_PROCESS => PROCESS,
        _ => &[],
    };
    (event.kind as usize).checked_sub(1).and_then(|index| names.get(index)).copied().unwrap_or(b"scheduler_event")
}

/// The word of a process event's `cause`: a terminal cause, or an admission
/// reason.
fn process_cause_word(event: &SchedulerTraceEvent) -> *const c_char {
    const TERMINAL: [&CStr; 5] = [c"returned", c"failed", c"defected", c"cancelled", c"forced"];
    const ADMISSION: [&CStr; 4] = [c"capacity_reached", c"name_taken", c"stopped", c"start_failed"];
    let words: &[&CStr] = match event.kind {
        Trace::PROCESS_TERMINATED => &TERMINAL,
        Trace::PROCESS_ADMISSION_REJECTED => &ADMISSION,
        _ => &[],
    };
    (event.cause as usize)
        .checked_sub(1)
        .and_then(|index| words.get(index))
        .map_or(ptr::null(), |word| word.as_ptr())
}

/// The process family's fields (supervised-process-conformance.md, "Runner
/// telemetry for process and clock events"): identities at the top level,
/// the rest in `attributes`, a field the kind does not set as null.
///
/// # Safety
/// `runtime` is the running runtime, with the trace lock held.
unsafe fn process_fields(out: &Out, runtime: *const Runtime, event: &SchedulerTraceEvent, attributes: bool) {
    if !attributes {
        out.identity(b"supervisor_id", event.supervisor);
        out.identity(b"membership_id", event.membership);
        out.identity(b"incarnation_id", event.incarnation);
        out.identity(b"parent_supervisor_id", event.parent_supervisor);
        return;
    }
    let send = event.kind == Trace::PROCESS_SEND;
    const OUTCOMES: [&CStr; 3] = [c"accepted", c"full", c"unavailable"];
    // SAFETY: the caller's contract; every word is a C string or null.
    unsafe {
        out.key(b"time_ns");
        out.signed(event.time);
        out.word(b"cause", process_cause_word(event));
        out.word(b"name", chimi_runtime_process_name_label_v1(event.name));
        let address: *const c_char = match event.address_kind {
            Trace::ADDRESS_NAMED if send => c"named".as_ptr(),
            Trace::ADDRESS_DIRECT if send => c"direct".as_ptr(),
            _ => ptr::null(),
        };
        out.word(b"address_kind", address);
        let outcome = if send { (event.outcome as usize).checked_sub(1).and_then(|index| OUTCOMES.get(index)) } else { None };
        out.word(b"outcome", outcome.map_or(ptr::null(), |word| word.as_ptr()));
        out.key(b"attempts_in_window");
        out.unsigned(event.attempts_in_window);
        // `defect`: why a defected incarnation ended. A stack overflow ends
        // the body without returning, so the scheduler reports it.
        let mut named: *const c_char = ptr::null();
        if event.kind == Trace::PROCESS_TERMINATED && event.cause == Trace::PROCESS_CAUSE_DEFECTED {
            if event.outcome == Trace::PROCESS_DEFECT_STACK_OVERFLOW {
                named = c"Process stack overflow".as_ptr();
            } else if (*runtime).last_defect_incarnation == event.incarnation {
                named = (*runtime).last_defect_message;
            }
        }
        out.word(b"defect", named);
    }
}

/// `id` written as an optional task or channel identity: null for 0.
fn optional(out: &Out, key: &[u8], id: u64, absent: bool) {
    out.key(key);
    if absent || id == 0 { out.bytes(b"null") } else { out.unsigned(id) }
}

/// The scheduler's trace hook: one of its task, channel, clock or process
/// events as a telemetry line.
///
/// # Safety
/// `context` is the running runtime; `event` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_trace_scheduler_event_v1(context: *mut c_void, event: *const SchedulerTraceEvent) {
    let runtime = context.cast::<Runtime>();
    // SAFETY: the caller's contract.
    unsafe {
        let event = &*event;
        let out = Out((*runtime).telemetry.cast());
        locked(|| {
            out.head(runtime, scheduler_event_name(event));
            optional(&out, b"task_id", event.task, false);
            optional(&out, b"parent_task_id", event.parent, false);
            // A clock event's `cause` is the waiter's request kind, and a
            // process event's is a cause code, not a task
            // (supervised-process-conformance.md, "Runner telemetry").
            let clock = event.family == Trace::FAMILY_CLOCK;
            let process = event.family == Trace::FAMILY_PROCESS;
            optional(&out, b"cause_task_id", event.cause, clock || process);
            optional(&out, b"channel_id", event.channel, false);
            if process {
                process_fields(&out, runtime, event, false);
            }
            out.bytes(b",\"attributes\":{\"scheduler_sequence\":");
            out.unsigned(event.sequence);
            for (key, value) in [
                (&b"capacity"[..], event.capacity),
                (b"queue_depth", event.queue_depth),
                (b"waiter_depth", event.waiter_depth),
                (b"discard_count", event.discard_count),
            ] {
                out.key(key);
                out.unsigned(value);
            }
            if clock {
                out.key(b"time_ns");
                out.signed(event.time);
                out.key(if event.kind == Trace::CLOCK_SELECT_COMMITTED { b"index" } else { b"request_kind" });
                out.unsigned(event.cause);
            }
            if process {
                process_fields(&out, runtime, event, true);
            }
            out.bytes(b"}}\n");
        });
    }
}
