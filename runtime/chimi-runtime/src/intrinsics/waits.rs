//! std/wait (time-and-outcomes.md, "Choosing among events"): the event
//! builders and `Wait.first`. Moved from `runtime.c` once Rust served the
//! clocks, channels and tasks it uses.
//!
//! An Event is a record that is never shown to source code:
//! - `kind` is an [`EventKind`] code.
//! - `source` is the receiver, inbox, task, Duration or Instant it waits on.
//! - `map` is the mapping function, or the value itself for a timer.
//! - `clock` is the provider a timer snapshotted when it was built, or Void
//!   for the execution clock.

use alloc::vec::Vec;
use core::ffi::CStr;

use super::channels::channel_operation;
use super::clocks::{ClockCall, clock_host_value, execution_now, overflow_defect};
use super::standard::{defect, shape_defect, success, void_value};
use super::tasks::{TaskCall, start, task_value};
use crate::eval::{cancelled, select_value};
use crate::heap::layout::{
    AllocationKind, Callable, CallableKind, Control, ControlKind, Core, Environment, Field, Intrinsic, Runtime,
    Scheduler, SchedulerRequest, SelectSource, Value, ValueKind,
};
use crate::heap::list;
use crate::heap::values::{allocate_kind, integer_value, new_environment, new_value};
use crate::scheduler::{self, cancel_is_terminal};

unsafe extern "C" {
    static CHIMI_CLOCK_REQUIREMENT_SLOT: u32;
    /// `Process.receive` on the calling process body's inbox.
    fn chimi_runtime_process_receive_v1(
        runtime: *mut Runtime,
        inbox: *mut Value,
        span: u64,
        call_node: u32,
        environment: *mut Environment,
    ) -> *mut Control;
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// An Event's `kind` field.
struct EventKind;

impl EventKind {
    const RECEIVE: i64 = 1;
    const INBOX: i64 = 2;
    const TASK: i64 = 3;
    const AFTER: i64 = 4;
    const AT: i64 = 5;
}

/// # Safety
/// `event` is an Event record.
unsafe fn field(event: *mut Value, name: &'static CStr) -> *mut Value {
    // SAFETY: the caller's contract; an Event has every field.
    unsafe { select_value(event, name.as_ptr()) }
}

/// # Safety
/// `event` is an Event record.
unsafe fn event_kind(event: *mut Value) -> i64 {
    // SAFETY: the caller's contract; `kind` is an Int.
    unsafe { (*field(event, c"kind")).payload.integer }
}

/// The Event record, in `kind, source, map, clock` order.
///
/// # Safety
/// Runs on a fiber that may allocate; the values are live.
unsafe fn event(kind: i64, source: *mut Value, map: *mut Value, clock: *mut Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::RECORD);
        let fields = allocate_kind::<Field>(4, AllocationKind::FIELDS);
        (*value).payload.record.length = 4;
        (*value).payload.record.fields = fields;
        *fields = Field { name: c"kind".as_ptr(), value: integer_value(ValueKind::INTEGER, kind) };
        *fields.add(1) = Field { name: c"source".as_ptr(), value: source };
        *fields.add(2) = Field { name: c"map".as_ptr(), value: map };
        *fields.add(3) = Field { name: c"clock".as_ptr(), value: clock };
        value
    }
}

/// # Safety
/// `value` is null or live.
unsafe fn is_host(value: *mut Value, host: u32) -> bool {
    // SAFETY: the caller's contract; a CALLABLE value's payload is a callable.
    unsafe {
        !value.is_null()
            && (*value).kind == ValueKind::CALLABLE
            && (*(*value).payload.callable).kind == CallableKind::HOST
            && (*(*value).payload.callable).host == host
    }
}

/// True when the provider's `now` and `sleep_until` are the execution
/// clock's, so its timers go straight onto the scheduler's timer queue.
/// `Clock.starting_at` shifts only `timestamp`, so it counts.
///
/// # Safety
/// `provider` is null or live.
unsafe fn on_execution_clock(provider: *mut Value) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        if provider.is_null() || (*provider).kind == ValueKind::VOID {
            return true;
        }
        (*provider).kind == ValueKind::RECORD
            && is_host(select_value(provider, c"now".as_ptr()), Core::HOST_CLOCK_NOW)
            && is_host(select_value(provider, c"sleep_until".as_ptr()), Core::HOST_CLOCK_SLEEP_UNTIL)
    }
}

/// A timer event's deadline, with the `now` it is measured against: `now`
/// for the execution clock (read once per `Wait.first`), or the event's own
/// provider's.
///
/// # Safety
/// Runs on a fiber that may allocate; `event` is a timer Event.
unsafe fn timer_deadline(call: ClockCall, event: *mut Value, now: i64) -> Result<(i64, i64), *mut Control> {
    // SAFETY: the caller's contract.
    unsafe {
        let node = i64::from(call.call_node);
        let source = (*field(event, c"source")).payload.integer;
        let provider = field(event, c"clock");
        let mut now = now;
        if !on_execution_clock(provider) {
            let read = select_value(provider, c"now".as_ptr());
            if read.is_null() || (*read).kind != ValueKind::CALLABLE {
                return Err(defect(c"provider has no callable field", node));
            }
            let control = call.call((*read).payload.callable, &mut []);
            if (*control).kind != ControlKind::SUCCESS {
                return Err(control);
            }
            if (*(*control).value).kind != ValueKind::INSTANT {
                return Err(shape_defect(call.call_node));
            }
            now = (*(*control).value).payload.integer;
        }
        if event_kind(event) == EventKind::AT {
            return Ok((now, source));
        }
        match now.checked_add(source) {
            Some(deadline) => Ok((now, deadline)),
            None => Err(overflow_defect(b"`Wait.after(", source, node)),
        }
    }
}

/// Starts the child that stands for a timer on a user Clock: it runs the
/// provider's `sleep_until(deadline)`, and the wait waits for it to end.
///
/// # Safety
/// Runs on a fiber of a scheduled program; `provider` is a Clock record.
unsafe fn start_user_timer(call: ClockCall, provider: *mut Value, deadline: i64) -> Result<u64, *mut Control> {
    // SAFETY: the caller's contract.
    unsafe {
        let sleep_until = select_value(provider, c"sleep_until".as_ptr());
        if sleep_until.is_null() || (*sleep_until).kind != ValueKind::CALLABLE {
            return Err(defect(c"provider has no callable field", i64::from(call.call_node)));
        }
        let sleeper = clock_host_value(Core::HOST_SELECT_SLEEP);
        let callable: *mut Callable = (*sleeper).payload.callable;
        (*callable).inner = (*sleep_until).payload.callable;
        (*callable).shift = deadline;
        let ClockCall { runtime, span, call_node, environment } = call;
        let started = start(TaskCall { runtime, span, call_node, environment }, sleeper);
        if (*started).kind != ControlKind::SUCCESS {
            return Err(started);
        }
        Ok((*(*started).value).payload.task)
    }
}

/// Cancels the user-clock timer children except the one at `keep`.
///
/// # Safety
/// Runs on a fiber of a scheduled program; each marked source names a child
/// this task started.
unsafe fn cancel_user_timers(sources: &[SelectSource], user_timer: &[bool], keep: Option<usize>, call_node: u32) {
    for (index, source) in sources.iter().enumerate() {
        if !user_timer[index] || keep == Some(index) {
            continue;
        }
        // SAFETY: the caller's contract.
        unsafe {
            channel_operation(Intrinsic::TASK_CANCEL, &[task_value(source.task)], call_node);
        }
    }
}

/// Takes the committed event's value with its source's own operation, which
/// answers at once:
/// - A receive source's value was already taken by the select and kept for
///   this task.
/// - An inbox or a task handle has this task as its only consumer.
/// - A timer's value is its event value.
///
/// Nothing may be issued between the select and this.
///
/// # Safety
/// Runs on a fiber of a scheduled program; `event` is an Event and
/// `user_timer_task` is 0 or the child standing for its timer.
unsafe fn take(call: ClockCall, event: *mut Value, user_timer_task: u64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let source = field(event, c"source");
        match event_kind(event) {
            EventKind::RECEIVE => channel_operation(Intrinsic::CHANNEL_RECEIVE, &[source], call.call_node),
            EventKind::INBOX => {
                chimi_runtime_process_receive_v1(call.runtime, source, call.span, call.call_node, call.environment)
            }
            EventKind::TASK => channel_operation(Intrinsic::TASK_RESULT, &[source], call.call_node),
            _ => {
                let map = field(event, c"map");
                if user_timer_task == 0 {
                    return success(map);
                }
                // Observe the timer child: a cancelled sleep cancels this
                // task, as it would a direct `sleep_until`.
                let slept = channel_operation(Intrinsic::TASK_RESULT, &[task_value(user_timer_task)], call.call_node);
                if (*slept).kind == ControlKind::SUCCESS { success(map) } else { slept }
            }
        }
    }
}

/// Maps a taken value with the providers the mapping function snapshotted
/// when the event was built. A timer has no mapping function.
///
/// # Safety
/// Runs on a fiber that may allocate; `event` is an Event.
unsafe fn map(call: ClockCall, event: *mut Value, taken: *mut Control) -> *mut Control {
    // SAFETY: the caller's contract; a non-timer Event's map is a Callable.
    unsafe {
        let kind = event_kind(event);
        if (*taken).kind != ControlKind::SUCCESS || kind == EventKind::AFTER || kind == EventKind::AT {
            return taken;
        }
        let call = ClockCall { environment: new_environment(0), ..call };
        call.call((*field(event, c"map")).payload.callable, &mut [(*taken).value])
    }
}

/// The scheduler's source for `event`, starting a child for a timer on a
/// user Clock. `now` is the execution clock's, read on the first timer.
///
/// # Safety
/// Runs on a fiber of a scheduled program; `event` is an Event.
unsafe fn select_source(
    call: ClockCall,
    event: *mut Value,
    now: &mut Option<i64>,
) -> Result<(SelectSource, bool), *mut Control> {
    // SAFETY: the caller's contract; payloads are read as the kind says.
    unsafe {
        let source = field(event, c"source");
        let mut entry = SelectSource::default();
        match event_kind(event) {
            EventKind::RECEIVE => {
                let capability = &(*source).payload.capability;
                entry.kind = Scheduler::SELECT_RECEIVE;
                entry.channel = capability.channel;
                entry.owner = capability.owner;
                entry.token = capability.token;
            }
            EventKind::INBOX => entry.kind = Scheduler::SELECT_INBOX,
            EventKind::TASK => {
                entry.kind = Scheduler::SELECT_TASK;
                entry.task = (*source).payload.task;
            }
            _ => {
                // Every timer measures from the start of this call.
                let start = match *now {
                    Some(start) => start,
                    None => *now.insert(execution_now(i64::from(call.call_node))?),
                };
                let (measured, deadline) = timer_deadline(call, event, start)?;
                let provider = field(event, c"clock");
                entry.kind = Scheduler::SELECT_TIMER;
                if on_execution_clock(provider) {
                    entry.deadline = deadline;
                } else if deadline <= measured {
                    // Already due on its own clock: ready now, in list order.
                    entry.deadline = i64::MIN;
                } else {
                    entry.kind = Scheduler::SELECT_TASK;
                    entry.task = start_user_timer(call, provider, deadline)?;
                    return Ok((entry, true));
                }
            }
        }
        Ok((entry, false))
    }
}

/// `Wait.first` (time-and-outcomes.md, "Semantics of `Wait.first`"): one
/// select request over every event in list order, reissued after each wake,
/// then the committed event's value. A timer on a user Clock is a child
/// running that clock's `sleep_until`, so it is a task source to the
/// scheduler.
///
/// # Safety
/// Runs on a fiber of a scheduled program; `events` is a List of Events.
unsafe fn first(call: ClockCall, events: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = i64::from(call.call_node);
        let count = (*events).payload.list.length;
        if count == 0 {
            return defect(c"`Wait.first` needs at least one event", node);
        }
        let mut sources: Vec<SelectSource> = Vec::new();
        let mut user_timer: Vec<bool> = Vec::new();
        if sources.try_reserve_exact(count as usize).is_err() || user_timer.try_reserve_exact(count as usize).is_err()
        {
            chimi_runtime_exhausted_memory();
        }
        let mut stopped: *mut Control = core::ptr::null_mut();
        let mut now = None;
        for index in 0..count {
            match select_source(call, list::get(events, index), &mut now) {
                Ok((source, user)) => {
                    sources.push(source);
                    user_timer.push(user);
                }
                Err(control) => {
                    stopped = control;
                    break;
                }
            }
        }
        let mut committed = None;
        while stopped.is_null() {
            let mut request = SchedulerRequest::of(Scheduler::REQUEST_SELECT);
            request.context = sources.as_mut_ptr().cast();
            request.value = i64::from(count);
            if scheduler::request(&mut request) {
                stopped = cancelled();
                break;
            }
            if request.status == Scheduler::RESPONSE_PENDING {
                continue;
            }
            if request.status != Scheduler::RESPONSE_OK || request.value < 0 || request.value >= i64::from(count) {
                stopped = defect(c"Wait.first scheduler request failed", node);
                break;
            }
            committed = Some(request.value as usize);
            break;
        }
        let mut taken = core::ptr::null_mut();
        if let Some(index) = committed {
            let timer_task = if user_timer[index] { sources[index].task } else { 0 };
            taken = take(call, list::get(events, index as u32), timer_task);
        }
        // Only after the take: a cancel reply lets other tasks run first.
        if stopped.is_null() || (*stopped).kind != ControlKind::CANCEL || !cancel_is_terminal() {
            cancel_user_timers(&sources, &user_timer, committed, call.call_node);
        }
        match committed {
            Some(index) if stopped.is_null() => map(call, list::get(events, index as u32), taken),
            _ => stopped,
        }
    }
}

/// The std/wait intrinsic `intrinsic` on `arguments`: an event builder or
/// `Wait.first`.
///
/// # Safety
/// Runs on a fiber that may allocate (of a scheduled program for
/// `Wait.first`); `arguments` are live and the call's fields are its own.
pub unsafe fn wait_operation(call: ClockCall, intrinsic: u32, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract; the slot is the program's.
    unsafe {
        let (source_kind, kind) = match intrinsic {
            // A watch subscription is a receiver: its event is a receive event.
            Intrinsic::SELECT_RECEIVE | Intrinsic::SELECT_WATCH => (ValueKind::CHANNEL_RECEIVER, EventKind::RECEIVE),
            Intrinsic::SELECT_INBOX => (ValueKind::PROCESS_INBOX, EventKind::INBOX),
            Intrinsic::SELECT_TASK => (ValueKind::TASK, EventKind::TASK),
            Intrinsic::SELECT_AFTER => (ValueKind::DURATION, EventKind::AFTER),
            Intrinsic::SELECT_AT => (ValueKind::INSTANT, EventKind::AT),
            Intrinsic::SELECT_FIRST => {
                if arguments.len() != 1 || (*arguments[0]).kind != ValueKind::LIST {
                    return shape_defect(call.call_node);
                }
                return first(call, arguments[0]);
            }
            _ => return defect(c"unknown standard intrinsic", i64::from(call.call_node)),
        };
        if arguments.len() != 2 || (*arguments[0]).kind != source_kind {
            return shape_defect(call.call_node);
        }
        let timer = kind == EventKind::AFTER || kind == EventKind::AT;
        if !timer && (*arguments[1]).kind != ValueKind::CALLABLE {
            return shape_defect(call.call_node);
        }
        let mut clock = core::ptr::null_mut();
        if timer {
            clock = *(*call.environment).requirements.add(CHIMI_CLOCK_REQUIREMENT_SLOT as usize);
        }
        if clock.is_null() {
            clock = void_value();
        }
        success(event(kind, arguments[0], arguments[1], clock))
    }
}
