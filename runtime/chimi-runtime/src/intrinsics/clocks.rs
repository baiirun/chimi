//! std/clock, `Task.timeout` and `Task.deadline` (time-and-outcomes.md,
//! "Time sources" and "Timeouts and deadlines"), with the execution clock
//! provider's host methods. Moved from `runtime.c` once Rust could make
//! scheduler requests (`crate::scheduler`).
//!
//! A `clock` provider bound by `using` sits in the clock requirement slot,
//! and every std/clock function reads time through it. With none bound, the
//! runner's execution clock answers: the scheduler's real or virtual source.
//! Its methods are `HOST_CLOCK_*` callables, and referencing the `clock` need
//! builds it as a record, so it can be passed on, for example to
//! `Clock.starting_at`.
//!
//! `Task.timeout` and `Task.deadline` read and wait on the execution time
//! source, not on a bound provider: a scope's deadline is a scheduler timer.

use core::ffi::{CStr, c_char};
use core::ptr;

use super::message::Message;
use super::standard::{defect, defect_with, failure, shape_defect, standard_value, success, void_value};
use crate::duration;
use crate::eval::{cancelled, invoke, select_value};
use crate::heap::layout::{
    AllocationKind, Callable, CallableKind, Control, ControlKind, Core, Environment, Field, Intrinsic, Runtime,
    Scheduler, SchedulerRequest, StandardConstructor, Value, ValueKind,
};
use crate::heap::values::{allocate_kind, integer_value, new_value, pointer_value, text_value};
use crate::scheduler::{self, cancel_is_terminal};
use crate::timestamp;

unsafe extern "C" {
    static CHIMI_CLOCK_REQUIREMENT_SLOT: u32;
}

/// The call a clock operation belongs to.
#[derive(Clone, Copy)]
pub struct ClockCall {
    pub runtime: *mut Runtime,
    pub span: u64,
    pub call_node: u32,
    pub environment: *mut Environment,
}

impl ClockCall {
    /// Calls `callable` with `arguments`. One copy of the inlined `invoke`
    /// serves every clock operation.
    ///
    /// # Safety
    /// Runs on a fiber; `callable` and `arguments` are live.
    #[inline(never)]
    unsafe fn call(self, callable: *mut Callable, arguments: &mut [*mut Value]) -> *mut Control {
        let values = if arguments.is_empty() { ptr::null_mut() } else { arguments.as_mut_ptr() };
        // SAFETY: the caller's contract.
        unsafe {
            invoke(self.runtime, callable, values, arguments.len() as u32, self.span, self.call_node, self.environment)
        }
    }

    fn node(self) -> i64 {
        i64::from(self.call_node)
    }
}

/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn instant(nanoseconds: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::INSTANT, nanoseconds) }
}

/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn timestamp_value(nanoseconds: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::TIMESTAMP, nanoseconds) }
}

/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn duration_value(nanoseconds: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::DURATION, nanoseconds) }
}

/// A host callable of the clock family.
///
/// # Safety
/// Runs on a fiber that may allocate.
pub(crate) unsafe fn clock_host_value(function: u32) -> *mut Value {
    // SAFETY: the caller's contract; the allocation is zeroed.
    unsafe {
        let callable = allocate_kind::<Callable>(1, AllocationKind::CALLABLE);
        (*callable).kind = CallableKind::HOST;
        (*callable).host = function;
        (*callable).name = c"clock".as_ptr();
        pointer_value(ValueKind::CALLABLE, callable.cast())
    }
}

/// A Clock record: `now`, `sleep_until`, `timestamp`, in field order.
///
/// # Safety
/// Runs on a fiber that may allocate; the values are live.
unsafe fn clock_record(now: *mut Value, sleep_until: *mut Value, timestamp: *mut Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::RECORD);
        let fields = allocate_kind::<Field>(3, AllocationKind::FIELDS);
        (*value).payload.record.length = 3;
        (*value).payload.record.fields = fields;
        *fields = Field { name: c"now".as_ptr(), value: now };
        *fields.add(1) = Field { name: c"sleep_until".as_ptr(), value: sleep_until };
        *fields.add(2) = Field { name: c"timestamp".as_ptr(), value: timestamp };
        value
    }
}

/// The runner's execution clock provider, as a record: the `clock` need's
/// value when none is bound.
///
/// # Safety
/// Runs on a fiber that may allocate.
pub(crate) unsafe fn execution_clock_value() -> *mut Value {
    // SAFETY: the caller's contract; the arguments are made in the C's order.
    unsafe {
        let now = clock_host_value(Core::HOST_CLOCK_NOW);
        let sleep_until = clock_host_value(Core::HOST_CLOCK_SLEEP_UNTIL);
        let timestamp = clock_host_value(Core::HOST_CLOCK_TIMESTAMP);
        clock_record(now, sleep_until, timestamp)
    }
}

/// The execution source's now; reading checks cancellation.
///
/// # Safety
/// Runs on a fiber of a scheduled program.
unsafe fn execution_now(node: i64) -> Result<i64, *mut Control> {
    let mut request = SchedulerRequest::of(Scheduler::REQUEST_TIMER_NOW);
    // SAFETY: the caller's contract.
    unsafe {
        if scheduler::request(&mut request) {
            return Err(cancelled());
        }
        if request.status != Scheduler::RESPONSE_OK {
            return Err(defect(c"Clock scheduler request failed", node));
        }
    }
    Ok(request.value)
}

/// Parks until `deadline`; at or before now it returns without parking.
///
/// # Safety
/// Runs on a fiber of a scheduled program.
unsafe fn execution_sleep_until(deadline: i64, failed: &'static CStr, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        loop {
            let mut request = SchedulerRequest::of(Scheduler::REQUEST_TIMER_SLEEP_UNTIL);
            request.value = deadline;
            if scheduler::request(&mut request) {
                return cancelled();
            }
            if request.status == Scheduler::RESPONSE_OK {
                return success(void_value());
            }
            if request.status != Scheduler::RESPONSE_PENDING {
                return defect(failed, node);
            }
        }
    }
}

/// Virtual wall time is `--clock-start` plus now, so both advance together;
/// real wall time is the host's.
///
/// # Safety
/// Runs on a fiber of a scheduled program.
unsafe fn execution_timestamp(runtime: *mut Runtime, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let now = match execution_now(node) {
            Ok(now) => now,
            Err(stopped) => return stopped,
        };
        if (*runtime).virtual_clock {
            let Some(wall) = (*runtime).clock_start.checked_add(now) else {
                return defect(c"`Clock.timestamp` overflows the Timestamp range", node);
            };
            return success(timestamp_value(wall));
        }
        let mut host = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        libc::clock_gettime(libc::CLOCK_REALTIME, &raw mut host);
        success(timestamp_value(host.tv_sec.wrapping_mul(1_000_000_000).wrapping_add(host.tv_nsec)))
    }
}

/// The execution clock's host methods, `Clock.starting_at`'s shifted
/// timestamp, and a wait's timer on a user Clock.
///
/// # Safety
/// Runs on a fiber of a scheduled program that may allocate; `callable` is
/// a HOST callable of the clock family and the arguments are live.
pub unsafe fn clock_host(call: ClockCall, callable: *mut Callable, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = call.node();
        match (*callable).host {
            Core::HOST_CLOCK_NOW => match execution_now(node) {
                Ok(now) => success(instant(now)),
                Err(stopped) => stopped,
            },
            Core::HOST_CLOCK_TIMESTAMP => execution_timestamp(call.runtime, node),
            Core::HOST_CLOCK_SLEEP_UNTIL => {
                if arguments.len() != 1 || (*arguments[0]).kind != ValueKind::INSTANT {
                    return shape_defect(call.call_node);
                }
                execution_sleep_until((*arguments[0]).payload.integer, c"Clock scheduler request failed", node)
            }
            Core::HOST_CLOCK_SHIFTED_TIMESTAMP => {
                let now = call.call((*callable).inner, &mut []);
                if (*now).kind != ControlKind::SUCCESS {
                    return now;
                }
                if (*(*now).value).kind != ValueKind::INSTANT {
                    return shape_defect(call.call_node);
                }
                match (*(*now).value).payload.integer.checked_add((*callable).shift) {
                    Some(wall) => success(timestamp_value(wall)),
                    None => defect(c"`Clock.starting_at` clock's timestamp overflows the Timestamp range", node),
                }
            }
            Core::HOST_SELECT_SLEEP => call.call((*callable).inner, &mut [instant((*callable).shift)]),
            _ => defect(c"unknown clock host function", node),
        }
    }
}

/// Calls `method` of the bound clock provider, or of the execution clock.
///
/// # Safety
/// Runs on a fiber that may allocate; the arguments are live.
unsafe fn clock_method(call: ClockCall, method: &'static CStr, arguments: &mut [*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract; the slot is the program's.
    unsafe {
        let mut provider = *(*call.environment).requirements.add(CHIMI_CLOCK_REQUIREMENT_SLOT as usize);
        if provider.is_null() {
            provider = execution_clock_value();
        }
        let callable = select_value(provider, method.as_ptr());
        if callable.is_null() || (*callable).kind != ValueKind::CALLABLE {
            return defect(c"provider has no callable field", call.node());
        }
        call.call((*callable).payload.callable, arguments)
    }
}

/// `Timeout.TimedOut(limit)`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn timed_out(limit: i64, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let error = standard_value(StandardConstructor::TIMEOUT_TIMED_OUT, &[duration_value(limit)]);
        if error.is_null() { defect(c"Timeout.TimedOut is missing from the program", node) } else { failure(error) }
    }
}

/// Runs `operation` in a cancellation scope of the calling task that the
/// scheduler fires at `deadline` (time-and-outcomes.md, "Semantics" of
/// timeouts):
/// - A deadline at or before now never starts the operation.
/// - The operation's own outcome stands: success, failure, defect, or a
///   cancellation that is not this scope's (the task's, an enclosing
///   scope's, or shutdown's).
/// - The scope's cancellation becomes `Timeout.TimedOut(limit)` here. The
///   scheduler says whether it is that cancellation when the scope exits; a
///   terminal cancellation exits nothing, because the fiber must not park
///   again.
///
/// # Safety
/// Runs on a fiber of a scheduled program; `operation` is a Callable.
unsafe fn run_in_scope(call: ClockCall, operation: *mut Value, now: i64, deadline: i64, limit: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = call.node();
        if deadline <= now {
            return timed_out(limit, node);
        }
        let mut enter = SchedulerRequest::of(Scheduler::REQUEST_SCOPE_ENTER);
        enter.value = deadline;
        if scheduler::request(&mut enter) {
            return cancelled();
        }
        if enter.status != Scheduler::RESPONSE_OK {
            return defect(c"Task.timeout scheduler request failed", node);
        }
        let control = call.call((*operation).payload.callable, &mut []);
        let was_cancelled = (*control).kind == ControlKind::CANCEL;
        if was_cancelled && cancel_is_terminal() {
            return control;
        }
        loop {
            // A cancelled operation's values are discarded, so the scope's
            // capabilities go.
            let mut exit = SchedulerRequest::of(Scheduler::REQUEST_SCOPE_EXIT);
            exit.token = enter.token;
            exit.value = i64::from(was_cancelled);
            if scheduler::request(&mut exit) {
                return cancelled();
            }
            if exit.status == Scheduler::RESPONSE_PENDING {
                continue;
            }
            if exit.status != Scheduler::RESPONSE_OK {
                return defect(c"Task.timeout scheduler request failed", node);
            }
            if was_cancelled && exit.value == 1 {
                return timed_out(limit, node);
            }
            return control;
        }
    }
}

/// The defect a sleep or timeout whose end is past the Instant range raises,
/// naming the call and its Duration.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn overflow_defect(call: &[u8], nanoseconds: i64, node: i64) -> *mut Control {
    let text = duration::text(nanoseconds);
    let message: *const c_char =
        Message::new().text(call).text(text.as_bytes()).text(b")` overflows the Instant range").finish();
    // SAFETY: the caller's contract.
    unsafe { defect_with(message, node) }
}

/// The intrinsic `intrinsic` of std/clock, or `Task.timeout` or
/// `Task.deadline`, on `arguments`.
///
/// # Safety
/// Runs on a fiber that may allocate (of a scheduled program for any that
/// reads the execution clock); `arguments` are live and the call's fields
/// are its own.
pub unsafe fn clock_operation(call: ClockCall, intrinsic: u32, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = call.node();
        let shape = |kinds: &[ValueKind]| {
            arguments.len() == kinds.len() && arguments.iter().zip(kinds).all(|(&value, &kind)| (*value).kind == kind)
        };
        let integer = |index: usize| (*arguments[index]).payload.integer;
        match intrinsic {
            Intrinsic::CLOCK_NOW if arguments.is_empty() => clock_method(call, c"now", &mut []),
            Intrinsic::CLOCK_TIMESTAMP if arguments.is_empty() => clock_method(call, c"timestamp", &mut []),
            Intrinsic::CLOCK_SLEEP_UNTIL if shape(&[ValueKind::INSTANT]) => {
                clock_method(call, c"sleep_until", &mut [arguments[0]])
            }
            Intrinsic::CLOCK_SINCE if shape(&[ValueKind::INSTANT]) => {
                let now = clock_method(call, c"now", &mut []);
                if (*now).kind != ControlKind::SUCCESS {
                    return now;
                }
                if (*(*now).value).kind != ValueKind::INSTANT {
                    return shape_defect(call.call_node);
                }
                match (*(*now).value).payload.integer.checked_sub(integer(0)) {
                    Some(result) => success(duration_value(result)),
                    None => defect(c"`Clock.since` overflows the Duration range", node),
                }
            }
            Intrinsic::CLOCK_ADD if shape(&[ValueKind::INSTANT, ValueKind::DURATION]) => {
                match integer(0).checked_add(integer(1)) {
                    Some(result) => success(instant(result)),
                    None => defect(c"`Clock.add` overflows the Instant range", node),
                }
            }
            Intrinsic::CLOCK_BETWEEN if shape(&[ValueKind::INSTANT, ValueKind::INSTANT]) => {
                match integer(1).checked_sub(integer(0)) {
                    Some(result) => success(duration_value(result)),
                    None => defect(c"`Clock.between` overflows the Duration range", node),
                }
            }
            Intrinsic::CLOCK_TIMESTAMP_TO_TEXT if shape(&[ValueKind::TIMESTAMP]) => {
                let text = timestamp::text(integer(0));
                let bytes = text.as_bytes();
                success(text_value(bytes.as_ptr().cast(), bytes.len()))
            }
            Intrinsic::CLOCK_UNIX_NANOSECONDS if shape(&[ValueKind::TIMESTAMP]) => {
                success(integer_value(ValueKind::INTEGER, integer(0)))
            }
            Intrinsic::CLOCK_FROM_UNIX_NANOSECONDS if shape(&[ValueKind::INTEGER]) => {
                success(timestamp_value(integer(0)))
            }
            Intrinsic::CLOCK_TIMESTAMP_BETWEEN if shape(&[ValueKind::TIMESTAMP, ValueKind::TIMESTAMP]) => {
                match integer(1).checked_sub(integer(0)) {
                    Some(result) => success(duration_value(result)),
                    None => defect(c"`Clock.timestamp_between` overflows the Duration range", node),
                }
            }
            Intrinsic::CLOCK_STARTING_AT if shape(&[ValueKind::RECORD, ValueKind::TIMESTAMP]) => {
                starting_at(call, arguments[0], integer(1))
            }
            Intrinsic::TASK_TIMEOUT if shape(&[ValueKind::CALLABLE, ValueKind::DURATION]) => {
                let now = match execution_now(node) {
                    Ok(now) => now,
                    Err(stopped) => return stopped,
                };
                let limit = integer(1);
                let Some(deadline) = now.checked_add(limit) else {
                    return overflow_defect(b"`Task.timeout(", limit, node);
                };
                run_in_scope(call, arguments[0], now, deadline, limit)
            }
            Intrinsic::TASK_DEADLINE if shape(&[ValueKind::CALLABLE, ValueKind::INSTANT]) => {
                let now = match execution_now(node) {
                    Ok(now) => now,
                    Err(stopped) => return stopped,
                };
                let deadline = integer(1);
                let Some(limit) = deadline.checked_sub(now) else {
                    return defect(c"`Task.deadline` is too far from now for a Duration", node);
                };
                run_in_scope(call, arguments[0], now, deadline, limit)
            }
            Intrinsic::CLOCK_SLEEP if shape(&[ValueKind::DURATION]) => sleep(call, integer(0)),
            _ => shape_defect(call.call_node),
        }
    }
}

/// `Clock.starting_at(clock, start)`: a clock whose timestamp starts at
/// `start` now and advances with `clock`'s now.
///
/// # Safety
/// As for [`clock_operation`]; `clock` is a record.
unsafe fn starting_at(call: ClockCall, clock: *mut Value, start: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = call.node();
        let now = select_value(clock, c"now".as_ptr());
        let sleep_until = select_value(clock, c"sleep_until".as_ptr());
        if now.is_null() || (*now).kind != ValueKind::CALLABLE || sleep_until.is_null() {
            return defect(c"provider has no callable field", node);
        }
        let started = call.call((*now).payload.callable, &mut []);
        if (*started).kind != ControlKind::SUCCESS {
            return started;
        }
        if (*(*started).value).kind != ValueKind::INSTANT {
            return shape_defect(call.call_node);
        }
        let Some(shift) = start.checked_sub((*(*started).value).payload.integer) else {
            return defect(c"`Clock.starting_at` overflows the Timestamp range", node);
        };
        let timestamp = clock_host_value(Core::HOST_CLOCK_SHIFTED_TIMESTAMP);
        let shifted = (*timestamp).payload.callable;
        (*shifted).inner = (*now).payload.callable;
        (*shifted).shift = shift;
        success(clock_record(now, sleep_until, timestamp))
    }
}

/// `Clock.sleep(d)` is `sleep_until(now + d)`. Reading now checks
/// cancellation; a deadline at or before now then returns without parking
/// (time-and-outcomes.md, "The Clock capability").
///
/// # Safety
/// As for [`clock_operation`].
unsafe fn sleep(call: ClockCall, nanoseconds: i64) -> *mut Control {
    // SAFETY: the caller's contract; the slot is the program's.
    unsafe {
        let node = call.node();
        if !(*(*call.environment).requirements.add(CHIMI_CLOCK_REQUIREMENT_SLOT as usize)).is_null() {
            // A bound provider: `sleep_until(add(now(), d))` through its methods.
            let read = clock_method(call, c"now", &mut []);
            if (*read).kind != ControlKind::SUCCESS {
                return read;
            }
            if (*(*read).value).kind != ValueKind::INSTANT {
                return shape_defect(call.call_node);
            }
            let Some(until) = (*(*read).value).payload.integer.checked_add(nanoseconds) else {
                return overflow_defect(b"`Clock.sleep(", nanoseconds, node);
            };
            return clock_method(call, c"sleep_until", &mut [instant(until)]);
        }
        let mut now = SchedulerRequest::of(Scheduler::REQUEST_TIMER_NOW);
        if scheduler::request(&mut now) {
            return cancelled();
        }
        if now.status != Scheduler::RESPONSE_OK {
            return defect(c"Clock.sleep scheduler request failed", node);
        }
        let Some(deadline) = now.value.checked_add(nanoseconds) else {
            return overflow_defect(b"`Clock.sleep(", nanoseconds, node);
        };
        execution_sleep_until(deadline, c"Clock.sleep scheduler request failed", node)
    }
}
