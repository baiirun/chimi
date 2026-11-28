//! Capability transfer for Task.start and the sends (CHI-305 step 5,
//! phase 8): the walk over the value graph is [`crate::heap::transfer`],
//! which calls [`chimi_runtime_transfer_capability_v1`] for each Receiver,
//! Completion and managed handle it meets. For a transfer, the child takes
//! each Receiver and Completion; when collecting for a send, each
//! capability found is recorded with the sender's token, and its copy in
//! the message takes the in-flight token (`ChimiCarriedCapability`).
//!
//! Also the counted handle values the channel and task intrinsics make: a
//! Task, Receiver, Completion or Sender value counts as a live reference to
//! its task or channel, in `runtime.c`'s handle counts.

use core::ffi::c_void;
use core::ptr;

use crate::heap::allocator::current_carrier;
use crate::heap::layout::{Capability, CarriedCapability, Scheduler, SchedulerRequest, Value, ValueKind};
use crate::heap::transfer;
use crate::heap::values::new_value;
use crate::scheduler;

unsafe extern "C" {
    /// Counts `value` as a live reference when it is a counted handle.
    fn chimi_runtime_retain_counted_value(value: *const Value);
    fn chimi_runtime_exhausted_memory() -> !;
}

/// `ChimiCarriedCapability.capability` (chimi_fiber.h).
const CAPABILITY_RECEIVER: u32 = 1;
const CAPABILITY_COMPLETION: u32 = 2;

/// One walk's state: a transfer to `child`, or (`collect`) a record of the
/// capabilities a message carries.
struct CapabilityTransfer {
    child: u64,
    /// Set when the task was cancelled during a transfer request.
    cancelled: bool,
    collect: bool,
    /// The collected capabilities, a `malloc` array the caller frees.
    carried: *mut CarriedCapability,
    carried_count: usize,
    carried_capacity: usize,
    /// Managed process and supervisor handles met while collecting. Each
    /// has one owner, so a value a broadcast or watch would copy must not
    /// hold one.
    managed_count: usize,
}

impl CapabilityTransfer {
    const fn new(child: u64, collect: bool) -> Self {
        Self {
            child,
            cancelled: false,
            collect,
            carried: ptr::null_mut(),
            carried_count: 0,
            carried_capacity: 0,
            managed_count: 0,
        }
    }

    /// Records `value`'s capability with the sender's token and moves the
    /// value's copy to the in-flight token.
    ///
    /// # Safety
    /// `value` is a live Receiver or Completion.
    unsafe fn collect(&mut self, value: *mut Value, capability: u32) {
        if self.carried_count == self.carried_capacity {
            let capacity = if self.carried_capacity == 0 { 8 } else { self.carried_capacity * 2 };
            let bytes = capacity.checked_mul(size_of::<CarriedCapability>());
            // SAFETY: `carried` is null or this walk's own malloc array.
            let carried = match bytes {
                Some(bytes) => unsafe { libc::realloc(self.carried.cast(), bytes) }.cast::<CarriedCapability>(),
                None => ptr::null_mut(),
            };
            if carried.is_null() {
                // SAFETY: no precondition.
                unsafe { chimi_runtime_exhausted_memory() };
            }
            self.carried = carried;
            self.carried_capacity = capacity;
        }
        // SAFETY: the caller's contract; the array has room for one more.
        unsafe {
            let held = &mut (*value).payload.capability;
            self.carried.add(self.carried_count).write(CarriedCapability {
                channel: held.channel,
                token: held.token,
                capability,
            });
            self.carried_count += 1;
            held.token = held.token.wrapping_add(1);
        }
    }

    /// Collects or transfers the Receiver or Completion `value`; false when
    /// the transfer failed or the task was cancelled meanwhile.
    ///
    /// # Safety
    /// Runs on a fiber; `value` is a live Receiver or Completion.
    unsafe fn captured(&mut self, value: *mut Value, request_kind: u32) -> bool {
        // SAFETY: the caller's contract.
        unsafe {
            if self.collect {
                let capability = if request_kind == Scheduler::REQUEST_CHANNEL_TRANSFER_RECEIVER {
                    CAPABILITY_RECEIVER
                } else {
                    CAPABILITY_COMPLETION
                };
                self.collect(value, capability);
                return true;
            }
            let held = &mut (*value).payload.capability;
            let mut request = SchedulerRequest::of(request_kind);
            request.task = self.child;
            request.channel = held.channel;
            request.owner = held.owner;
            request.token = held.token;
            if scheduler::request(&mut request) {
                self.cancelled = true;
                return false;
            }
            if request.status != Scheduler::RESPONSE_OK {
                return false;
            }
            let held = &mut (*value).payload.capability;
            held.owner = request.owner;
            held.token = request.token;
            true
        }
    }

    /// Walks the graph under `root`; false when a transfer request fails,
    /// or when the task is cancelled meanwhile (`cancelled` is then set).
    /// The walk's probes count on the carrier it ends on, since a transfer
    /// request can move the task to another carrier.
    ///
    /// # Safety
    /// Runs on a fiber; `root` is live.
    unsafe fn walk(&mut self, root: *mut Value) -> bool {
        let mut probes = 0;
        // SAFETY: the caller's contract; `self` outlives the walk.
        let transferred = unsafe { transfer::captures((self as *mut Self).cast(), root, &mut probes) };
        // SAFETY: the running carrier is this thread's.
        unsafe {
            let carrier = current_carrier();
            (*carrier).capture_transfer_probes = (*carrier).capture_transfer_probes.wrapping_add(probes);
        }
        transferred
    }
}

/// Called by the walk for each capability value it meets. False when a
/// transfer request fails, or when the task is cancelled meanwhile; the
/// walk then stops.
///
/// # Safety
/// `transfer` is the walk's [`CapabilityTransfer`]; `value` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_transfer_capability_v1(transfer: *mut c_void, value: *mut Value) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        let transfer = &mut *transfer.cast::<CapabilityTransfer>();
        match (*value).kind {
            ValueKind::CHANNEL_RECEIVER => transfer.captured(value, Scheduler::REQUEST_CHANNEL_TRANSFER_RECEIVER),
            ValueKind::CHANNEL_COMPLETION => transfer.captured(value, Scheduler::REQUEST_CHANNEL_TRANSFER_COMPLETION),
            ValueKind::MANAGED_PROCESS | ValueKind::MANAGED_SUPERVISOR => {
                if transfer.collect {
                    transfer.managed_count += 1;
                }
                true
            }
            _ => false,
        }
    }
}

/// Transfers the exclusive capabilities under `root` to the child task
/// `child`; false (with `*cancelled` when this task was cancelled
/// meanwhile) when one cannot move. Task.start calls it, and runtime.c's
/// task entry for a result.
///
/// # Safety
/// Runs on a fiber; `root` is live and `cancelled` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_transfer_to_child_v1(root: *mut Value, child: u64, cancelled: *mut bool) -> bool {
    let mut transfer = CapabilityTransfer::new(child, false);
    // SAFETY: the caller's contract.
    unsafe {
        let transferred = transfer.walk(root);
        *cancelled = transfer.cancelled;
        transferred
    }
}

/// The Receivers and Completions a message under `root` carries, collected
/// as for a send: a `malloc` list the caller frees, with its length and the
/// number of managed handles met.
///
/// # Safety
/// Runs on a fiber; `root` is a live copy the message will carry.
pub(crate) unsafe fn carried_capabilities(root: *mut Value) -> (*mut CarriedCapability, usize, usize) {
    let mut carried = CapabilityTransfer::new(0, true);
    // SAFETY: the caller's contract.
    unsafe { carried.walk(root) };
    (carried.carried, carried.carried_count, carried.managed_count)
}

/// Whether `root` reaches a Receiver or Completion: a std/test Mailbox
/// refuses a message with one (intrinsics/test_log.rs), since only a
/// process receive makes the capability transfer it needs. Collecting marks
/// the copies' tokens in flight, so `root` must be a detached copy the
/// caller frees.
///
/// # Safety
/// Runs on a fiber; `root` is live.
pub(crate) unsafe fn carries_capability(root: *mut Value) -> bool {
    // SAFETY: the caller's contract.
    let (items, count, _) = unsafe { carried_capabilities(root) };
    // SAFETY: the list came from malloc, or is null.
    unsafe { libc::free(items.cast()) };
    count != 0
}

/// A Receiver, Completion or Sender value of `kind`, counted as a live
/// reference to its channel.
///
/// # Safety
/// Runs on a fiber that may allocate.
pub(crate) unsafe fn capability_value(kind: ValueKind, channel: u64, owner: u64, token: u64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(kind);
        (*value).payload.capability = Capability { channel, owner, token };
        chimi_runtime_retain_counted_value(value);
        value
    }
}

/// A Task handle value, counted as a live reference to its task.
///
/// # Safety
/// Runs on a fiber that may allocate.
pub(crate) unsafe fn task_value(raw: u64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::TASK);
        (*value).payload.task = raw;
        chimi_runtime_retain_counted_value(value);
        value
    }
}
