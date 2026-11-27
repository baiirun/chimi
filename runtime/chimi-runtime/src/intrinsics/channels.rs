//! std/channel, std/broadcast and std/watch, with `Task.result` and
//! `Task.cancel`: the intrinsics that are one scheduler request, or a loop
//! of them, over capability values. Moved from `runtime.c`'s
//! `invoke_intrinsic` once Rust could make scheduler requests
//! (`crate::scheduler`).
//!
//! A message travels as a fragment. Once a channel accepts it, the
//! scheduler owns it and releases it if it is discarded; one the channel
//! refuses comes back, and the sender frees it and keeps its own original
//! value. Receivers and Completions in a message travel with it: the
//! scheduler puts them in flight, and the message's copies take the
//! in-flight token before anything can receive them.
//!
//! The carrier can change across a request, so each fragment operation
//! reads it again.

use core::ffi::{CStr, c_void};
use core::ptr;

use super::abi::arguments;
use super::message::Message;
use super::standard::{defect, defect_with, failure, shape_defect, standard_value, success, void_value};
use crate::eval::cancelled;
use crate::heap::allocator::current_carrier;
use crate::heap::fragment;
use crate::heap::layout::{
    AllocationKind, Control, Field, HeapStatus, Intrinsic, MessageCapabilities, RuntimeFragment, Scheduler,
    SchedulerRequest, StandardConstructor, Value, ValueKind, FRAGMENT_ROOTS,
};
use crate::heap::values::{allocate_kind, integer_value, new_value};
use crate::scheduler;

unsafe extern "C" {
    /// A capability value of `kind`, counted as a live reference to its
    /// channel (`runtime.c`, which keeps the handle counts).
    fn chimi_runtime_capability_value_v1(kind: u32, channel: u64, owner: u64, token: u64) -> *mut Value;
    /// The Receivers and Completions under `root`, collected as for a send;
    /// the caller frees the list.
    fn chimi_runtime_carried_capabilities_v1(root: *mut Value, count: *mut usize, managed: *mut usize) -> *mut c_void;
    /// The control a task's result word names.
    fn chimi_runtime_task_reply_v1(raw: i64) -> *mut Control;
}

/// The defect a copy reports when a value points into another heap.
const FOREIGN_POINTER_DEFECT: &CStr = c"Chimi runtime heap invariant violated: a value points into another heap";

/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn capability(kind: ValueKind, channel: u64, owner: u64, token: u64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { chimi_runtime_capability_value_v1(kind.0, channel, owner, token) }
}

/// A record of `fields`, in order.
///
/// # Safety
/// Runs on a fiber that may allocate; each value is live.
unsafe fn record(fields: &[(&'static CStr, *mut Value)]) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::RECORD);
        let slots = allocate_kind::<Field>(fields.len(), AllocationKind::FIELDS);
        (*value).payload.record.length = fields.len() as u32;
        (*value).payload.record.fields = slots;
        for (index, &(name, field)) in fields.iter().enumerate() {
            *slots.add(index) = Field { name: name.as_ptr(), value: field };
        }
        value
    }
}

/// The `{ completion, receiver, sender }` record of a new channel. The
/// record and its fields are allocated before the capabilities, as the C
/// did.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn channel_bundle(channel: u64, owner: u64, token: u64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = record(&[(c"completion", ptr::null_mut()), (c"receiver", ptr::null_mut()), (c"sender", ptr::null_mut())]);
        let fields = (*value).payload.record.fields;
        (*fields).value = capability(ValueKind::CHANNEL_COMPLETION, channel, owner, token);
        (*fields.add(1)).value = capability(ValueKind::CHANNEL_RECEIVER, channel, owner, token);
        (*fields.add(2)).value = capability(ValueKind::CHANNEL_SENDER, channel, 0, 0);
        value
    }
}

/// The `{ completion, publisher }` record of a broadcast or watch: its
/// publisher is a Sender of the hub's id.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn publisher_bundle(channel: u64, owner: u64, token: u64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = record(&[(c"completion", ptr::null_mut()), (c"publisher", ptr::null_mut())]);
        let fields = (*value).payload.record.fields;
        (*fields).value = capability(ValueKind::CHANNEL_COMPLETION, channel, owner, token);
        (*fields.add(1)).value = capability(ValueKind::CHANNEL_SENDER, channel, 0, 0);
        value
    }
}

/// A one-root fragment holding a copy of `value`, or `None` when it points
/// into another heap.
///
/// # Safety
/// Runs on a fiber; `value` is live.
unsafe fn fragment_of(value: *mut Value) -> Option<*mut RuntimeFragment> {
    // SAFETY: the caller's contract.
    unsafe {
        let roots = [value.cast::<c_void>()];
        let kinds = [AllocationKind::VALUE];
        let mut out = ptr::null_mut();
        let status = fragment::copy(current_carrier(), roots.as_ptr(), kinds.as_ptr(), 1, &raw mut out);
        (status == HeapStatus::OK).then_some(out)
    }
}

/// # Safety
/// `fragment` is live and unadopted.
unsafe fn free_fragment(fragment: *mut RuntimeFragment) {
    // SAFETY: the caller's contract.
    unsafe { fragment::free(current_carrier(), fragment) }
}

/// The capabilities a message's copy carries.
struct Carried {
    items: *mut c_void,
    count: usize,
    managed: usize,
}

impl Carried {
    /// # Safety
    /// Runs on a fiber; `root` is live.
    unsafe fn of(root: *mut c_void) -> Self {
        let (mut count, mut managed) = (0, 0);
        // SAFETY: the caller's contract.
        let items = unsafe { chimi_runtime_carried_capabilities_v1(root.cast(), &raw mut count, &raw mut managed) };
        Self { items, count, managed }
    }

    fn release(&mut self) {
        // SAFETY: the list came from malloc, or is null.
        unsafe { libc::free(self.items) };
        self.items = ptr::null_mut();
    }
}

/// The defects a publisher raises for a message with a managed process or
/// supervisor handle: every subscriber would get a copy.
const BROADCAST_MANAGED_DEFECT: &CStr = c"channel capability violation: Broadcast.publish cannot hold a managed process or supervisor handle, because every subscriber gets a copy";
const WATCH_MANAGED_DEFECT: &CStr = c"channel capability violation: Watch.publish cannot hold a managed process or supervisor handle, because every subscriber gets a copy";
const WATCH_INITIAL_MANAGED_DEFECT: &CStr = c"channel capability violation: Watch.starting_with cannot hold a managed process or supervisor handle, because every subscriber gets a copy";

/// # Safety
/// `value` is live.
unsafe fn is(value: *mut Value, kind: ValueKind) -> bool {
    // SAFETY: the caller's contract.
    unsafe { (*value).kind == kind }
}

/// A request of `kind` naming `value`'s channel, owner and token.
///
/// # Safety
/// `value` is a live capability.
unsafe fn capability_request(kind: u32, value: *mut Value) -> SchedulerRequest {
    let mut request = SchedulerRequest::of(kind);
    // SAFETY: the caller's contract.
    let capability = unsafe { (*value).payload.capability };
    request.channel = capability.channel;
    request.owner = capability.owner;
    request.token = capability.token;
    request
}

/// Sends `request` and gives Void on OK, or the defect `failed` names.
///
/// # Safety
/// Runs on a fiber of a scheduled program.
unsafe fn void_reply(mut request: SchedulerRequest, failed: &'static CStr, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if scheduler::request(&mut request) {
            return cancelled();
        }
        if request.status == Scheduler::RESPONSE_OK { success(void_value()) } else { defect(failed, node) }
    }
}

/// The intrinsic `intrinsic` of std/channel, std/broadcast, std/watch or
/// `Task.result`/`Task.cancel` on `arguments`.
///
/// # Safety
/// Runs on a fiber of a scheduled program that may allocate; `arguments`
/// are live.
pub unsafe fn channel_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let one = |kind: ValueKind| arguments.len() == 1 && is(arguments[0], kind);
        match intrinsic {
            Intrinsic::TASK_RESULT if one(ValueKind::TASK) => loop {
                let mut request = SchedulerRequest::of(Scheduler::REQUEST_TASK_RESULT);
                request.task = (*arguments[0]).payload.task;
                if scheduler::request(&mut request) {
                    return cancelled();
                }
                match request.status {
                    Scheduler::RESPONSE_PENDING => continue,
                    Scheduler::RESPONSE_SUCCEEDED => return chimi_runtime_task_reply_v1(request.value),
                    Scheduler::RESPONSE_CANCELLED => return cancelled(),
                    Scheduler::RESPONSE_DEFECTED if request.value > 0 => return chimi_runtime_task_reply_v1(request.value),
                    Scheduler::RESPONSE_STACK_OVERFLOW => return defect(c"Task stack overflow", node),
                    _ => return defect(c"Task.result scheduler request failed", node),
                }
            },
            Intrinsic::TASK_CANCEL if one(ValueKind::TASK) => {
                let mut request = SchedulerRequest::of(Scheduler::REQUEST_TASK_CANCEL);
                request.task = (*arguments[0]).payload.task;
                void_reply(request, c"Task.cancel scheduler request failed", node)
            }
            Intrinsic::CHANNEL_BOUNDED if one(ValueKind::INTEGER) && (*arguments[0]).payload.integer >= 0 => {
                let mut request = SchedulerRequest::of(Scheduler::REQUEST_CHANNEL_BOUNDED);
                request.value = (*arguments[0]).payload.integer;
                if scheduler::request(&mut request) {
                    return cancelled();
                }
                if request.status == Scheduler::RESPONSE_OK {
                    success(channel_bundle(request.channel, request.owner, request.token))
                } else {
                    defect(c"Channel.bounded scheduler request failed", node)
                }
            }
            Intrinsic::CHANNEL_ADD_RECEIVER if one(ValueKind::CHANNEL_RECEIVER) => {
                let mut request = capability_request(Scheduler::REQUEST_CHANNEL_ADD_RECEIVER, arguments[0]);
                if scheduler::request(&mut request) {
                    return cancelled();
                }
                if request.status == Scheduler::RESPONSE_OK {
                    success(capability(ValueKind::CHANNEL_RECEIVER, request.channel, request.owner, request.token))
                } else {
                    defect(c"Channel.add_receiver scheduler request failed", node)
                }
            }
            Intrinsic::BROADCAST_SUBSCRIBE | Intrinsic::WATCH_SUBSCRIBE if one(ValueKind::CHANNEL_SENDER) => {
                let mut request = SchedulerRequest::of(Scheduler::REQUEST_CHANNEL_SUBSCRIBE);
                request.channel = (*arguments[0]).payload.capability.channel;
                if scheduler::request(&mut request) {
                    return cancelled();
                }
                if request.status == Scheduler::RESPONSE_OK {
                    success(capability(ValueKind::CHANNEL_RECEIVER, request.channel, request.owner, request.token))
                } else {
                    defect(c"subscribe scheduler request failed", node)
                }
            }
            Intrinsic::BROADCAST_BOUNDED if one(ValueKind::INTEGER) => {
                let capacity = (*arguments[0]).payload.integer;
                if capacity < 1 {
                    let message = Message::new()
                        .text(b"`Broadcast.bounded(")
                        .integer(capacity)
                        .text(b")` needs a positive capacity")
                        .finish();
                    return defect_with(message, node);
                }
                let mut request = SchedulerRequest::of(Scheduler::REQUEST_CHANNEL_BROADCAST);
                request.value = capacity;
                if scheduler::request(&mut request) {
                    return cancelled();
                }
                if request.status == Scheduler::RESPONSE_OK {
                    success(publisher_bundle(request.channel, request.owner, request.token))
                } else {
                    defect(c"Broadcast.bounded scheduler request failed", node)
                }
            }
            Intrinsic::WATCH_STARTING_WITH if arguments.len() == 1 => watch_starting_with(arguments[0], node),
            Intrinsic::BROADCAST_PUBLISH | Intrinsic::WATCH_PUBLISH | Intrinsic::CHANNEL_SEND
                if arguments.len() == 2 && is(arguments[0], ValueKind::CHANNEL_SENDER) =>
            {
                send(intrinsic, arguments[0], arguments[1], node)
            }
            Intrinsic::WATCH_RECEIVE | Intrinsic::CHANNEL_RECEIVE if one(ValueKind::CHANNEL_RECEIVER) => {
                receive(arguments[0], node)
            }
            Intrinsic::CHANNEL_CLOSE if one(ValueKind::CHANNEL_COMPLETION) => void_reply(
                capability_request(Scheduler::REQUEST_CHANNEL_CLOSE, arguments[0]),
                c"Channel.close scheduler request failed",
                node,
            ),
            Intrinsic::CHANNEL_ABORT if one(ValueKind::CHANNEL_RECEIVER) => void_reply(
                capability_request(Scheduler::REQUEST_CHANNEL_ABORT, arguments[0]),
                c"Channel.abort scheduler request failed",
                node,
            ),
            _ => shape_defect(call_node),
        }
    }
}

/// `Watch.starting_with(initial)`: a watch whose subscribers first see a
/// copy of `initial`, which therefore cannot hold a capability that has one
/// owner.
///
/// # Safety
/// As for [`channel_operation`].
unsafe fn watch_starting_with(value: *mut Value, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let Some(initial) = fragment_of(value) else { return defect(FOREIGN_POINTER_DEFECT, node) };
        let mut carried = Carried::of((*initial).roots[0]);
        let (count, managed) = (carried.count, carried.managed);
        carried.release();
        if count != 0 || managed != 0 {
            free_fragment(initial);
            return defect(
                if count != 0 {
                    c"channel capability violation: Watch.starting_with cannot hold a value with a Receiver or Completion, because every subscriber gets a copy"
                } else {
                    WATCH_INITIAL_MANAGED_DEFECT
                },
                node,
            );
        }
        let mut request = SchedulerRequest::of(Scheduler::REQUEST_CHANNEL_WATCH);
        request.value = initial as i64;
        let cancel = scheduler::request(&mut request);
        // The scheduler takes the initial value only with an OK reply.
        if request.status != Scheduler::RESPONSE_OK {
            free_fragment(initial);
        }
        if cancel {
            return cancelled();
        }
        if request.status == Scheduler::RESPONSE_OK {
            success(publisher_bundle(request.channel, request.owner, request.token))
        } else {
            defect(c"Watch.starting_with scheduler request failed", node)
        }
    }
}

/// `Channel.send`, and `Broadcast.publish` and `Watch.publish`, whose
/// publisher is a Sender of its hub: the scheduler publishes a send to one,
/// and defects the task if the message carries a Receiver or Completion,
/// which every subscriber's copy would duplicate.
///
/// # Safety
/// As for [`channel_operation`]; `sender` is a Sender.
unsafe fn send(intrinsic: u32, sender: *mut Value, value: *mut Value, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let Some(message) = fragment_of(value) else { return defect(FOREIGN_POINTER_DEFECT, node) };
        let mut carried = Carried::of((*message).roots[0]);
        if carried.managed != 0 && intrinsic != Intrinsic::CHANNEL_SEND {
            carried.release();
            free_fragment(message);
            let message = if intrinsic == Intrinsic::BROADCAST_PUBLISH { BROADCAST_MANAGED_DEFECT } else { WATCH_MANAGED_DEFECT };
            return defect(message, node);
        }
        let mut capabilities = MessageCapabilities { items: carried.items, count: carried.count };
        let mut first = true;
        loop {
            let mut request = SchedulerRequest::of(Scheduler::REQUEST_CHANNEL_SEND);
            request.channel = (*sender).payload.capability.channel;
            request.value = message as i64;
            request.context = if first && capabilities.count != 0 {
                (&raw mut capabilities).cast()
            } else {
                ptr::null_mut()
            };
            first = false;
            let cancel = scheduler::request(&mut request);
            // The scheduler takes the message on OK or PENDING and hands it
            // back on CLOSED or ABORTED; any other reply leaves it with the
            // sender. Either way the sender frees it, even when it is being
            // cancelled.
            let taken = matches!(request.status, Scheduler::RESPONSE_OK | Scheduler::RESPONSE_PENDING);
            let returned = matches!(request.status, Scheduler::RESPONSE_CLOSED | Scheduler::RESPONSE_ABORTED);
            if returned {
                free_fragment(request.value as *mut RuntimeFragment);
            } else if !taken {
                free_fragment(message);
            }
            carried.release();
            if cancel {
                return cancelled();
            }
            match request.status {
                Scheduler::RESPONSE_PENDING => continue,
                Scheduler::RESPONSE_OK => {
                    return success(if intrinsic == Intrinsic::BROADCAST_PUBLISH {
                        integer_value(ValueKind::INTEGER, request.value)
                    } else {
                        void_value()
                    });
                }
                Scheduler::RESPONSE_CLOSED => {
                    return failure(standard_value(StandardConstructor::CHANNEL_SEND_CLOSED, &[value]));
                }
                Scheduler::RESPONSE_ABORTED => {
                    return failure(standard_value(StandardConstructor::CHANNEL_SEND_ABORTED, &[value]));
                }
                _ => return defect(c"Channel.send scheduler request failed", node),
            }
        }
    }
}

/// `Channel.receive`, and `Watch.receive`: a watch subscription is a
/// receiver of a queue that keeps only the latest value.
///
/// # Safety
/// As for [`channel_operation`]; `receiver` is a Receiver.
unsafe fn receive(receiver: *mut Value, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        loop {
            let mut request = capability_request(Scheduler::REQUEST_CHANNEL_RECEIVE, receiver);
            if scheduler::request(&mut request) {
                if request.status == Scheduler::RESPONSE_SUCCEEDED {
                    free_fragment(request.value as *mut RuntimeFragment);
                }
                return cancelled();
            }
            match request.status {
                Scheduler::RESPONSE_PENDING => continue,
                Scheduler::RESPONSE_SUCCEEDED => {
                    let mut roots = [ptr::null_mut::<c_void>(); FRAGMENT_ROOTS];
                    fragment::adopt(current_carrier(), request.value as *mut RuntimeFragment, roots.as_mut_ptr());
                    let received = standard_value(StandardConstructor::CHANNEL_RECEIVED, &[roots[0].cast()]);
                    return success(received);
                }
                Scheduler::RESPONSE_DRAINED => {
                    return success(standard_value(StandardConstructor::CHANNEL_DRAINED, &[]));
                }
                Scheduler::RESPONSE_ABORTED => {
                    return failure(standard_value(StandardConstructor::CHANNEL_ABORTED, &[]));
                }
                _ => return defect(c"Channel.receive scheduler request failed", node),
            }
        }
    }
}

/// [`channel_operation`] for `runtime.c`'s own callers: the waits take a
/// committed event's value with `Channel.receive` or `Task.result`, and
/// cancel their timer children with `Task.cancel`.
///
/// # Safety
/// As for [`channel_operation`]; `values` holds `count` live values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_channel_operation_v1(
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { channel_operation(intrinsic, arguments(values, count), call_node) }
}
