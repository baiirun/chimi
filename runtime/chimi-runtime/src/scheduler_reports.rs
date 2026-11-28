//! The messages the scheduler's reports leave for the root's outcome (CHI-305
//! step 5, phase 8): capability violations
//! (task-and-channel-capabilities.md, "Checked at run time"), a virtual-time
//! deadlock (time-and-outcomes.md, "Virtual time"), and the latest process
//! defect. The scheduler calls the report functions from whichever carrier
//! holds its kernel lock, with no task running there; other carriers and the
//! stop-signal handler may touch other `Runtime` fields meanwhile, so the
//! functions write their fields through the raw pointer and never borrow the
//! whole record. runtime.c reads the messages after the carriers are joined.

use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_void};
use core::ptr;

use crate::heap::layout::{CapabilityViolation, DeadlockWaiter, ProcessTermination, Runtime, Scheduler};
use crate::telemetry::unsigned_spelling;

unsafe extern "C" {
    fn chimi_runtime_exhausted_memory() -> !;
}

/// A message being built.
struct Text(Vec<u8>);

impl Text {
    fn new() -> Self {
        Self(Vec::new())
    }

    // Kept out of line: the messages call it dozens of times, and inlining
    // each call costs every program several KB.
    #[inline(never)]
    fn push(&mut self, bytes: &[u8]) -> &mut Self {
        if self.0.try_reserve(bytes.len()).is_err() {
            // SAFETY: exits the process.
            unsafe { chimi_runtime_exhausted_memory() }
        }
        self.0.extend_from_slice(bytes);
        self
    }

    #[inline(never)]
    fn number(&mut self, value: u64) -> &mut Self {
        let mut digits = [0; 20];
        self.push(unsigned_spelling(value, &mut digits))
    }

    /// The text as a NUL-terminated string from `malloc`, which C may free.
    fn into_c(self) -> *mut c_char {
        // SAFETY: the buffer holds the text and its NUL.
        unsafe {
            let raw = allocate(self.0.len() + 1).cast::<u8>();
            ptr::copy_nonoverlapping(self.0.as_ptr(), raw, self.0.len());
            *raw.add(self.0.len()) = 0;
            raw.cast()
        }
    }
}

/// `size` bytes from `malloc`; running out ends the program with exit 70,
/// as every runtime allocation failure does.
fn allocate(size: usize) -> *mut c_void {
    // SAFETY: malloc has no preconditions.
    let raw = unsafe { libc::malloc(size) };
    if raw.is_null() {
        // SAFETY: exits the process.
        unsafe { chimi_runtime_exhausted_memory() }
    }
    raw
}

/// The operation a parked task waits on, by its scheduler request kind.
fn waited_operation(request_kind: u32) -> &'static [u8] {
    match request_kind {
        Scheduler::REQUEST_TASK_RESULT => b"Task.result",
        Scheduler::REQUEST_TASK_ANY => b"Task.all, Task.results or Task.race",
        Scheduler::REQUEST_TASK_DRAIN => b"draining a Task combinator's children",
        Scheduler::REQUEST_CHANNEL_SEND => b"Channel.send",
        Scheduler::REQUEST_CHANNEL_RECEIVE => b"Channel.receive",
        Scheduler::REQUEST_SELECT => b"Wait.first",
        Scheduler::REQUEST_SIGNAL_WAIT => b"Signal.next",
        _ => b"a scheduler request",
    }
}

fn capability_name(capability: u32) -> &'static [u8] {
    match capability {
        CapabilityViolation::RECEIVER => b"Receiver",
        CapabilityViolation::COMPLETION => b"Completion",
        _ => b"capability",
    }
}

/// The surface operation behind a channel request kind.
fn channel_operation(request_kind: u32) -> &'static [u8] {
    match request_kind {
        Scheduler::REQUEST_CHANNEL_SEND => b"Channel.send",
        Scheduler::REQUEST_CHANNEL_RECEIVE => b"Channel.receive",
        Scheduler::REQUEST_CHANNEL_CLOSE => b"Channel.close",
        Scheduler::REQUEST_CHANNEL_ABORT => b"aborting the channel",
        Scheduler::REQUEST_CHANNEL_RELEASE => b"releasing the channel",
        Scheduler::REQUEST_CHANNEL_ADD_RECEIVER => b"Channel.add_receiver",
        Scheduler::REQUEST_CHANNEL_SUBSCRIBE => b"subscribe",
        Scheduler::REQUEST_PROCESS_SEND => b"Process.send",
        Scheduler::REQUEST_SUPERVISOR_START_PROCESS => b"Supervisor.start_handoff",
        _ => b"a channel request",
    }
}

/// A task the scheduler defected for a capability violation, and the defect
/// message naming the capability, the task and the capability's owner.
struct ViolationNote {
    task: u64,
    message: *mut c_char,
    next: *mut ViolationNote,
}

/// The message of a task using a Task handle it does not own, or one naming
/// no task. The handle travelled where the checker does not follow it, for
/// example inside a closure held by an interface value (CHI-311, CHI-316).
fn task_violation_text(violation: &CapabilityViolation) -> Text {
    let mut text = Text::new();
    text.push(b"task capability violation: task ").number(violation.task).push(b" cannot ");
    text.push(match violation.request_kind {
        Scheduler::REQUEST_TASK_RESULT => b"read the result of task ",
        Scheduler::REQUEST_TASK_CANCEL => b"cancel task ",
        _ => b"wait for task ",
    });
    text.number(violation.destination).push(b": ");
    if violation.reason == CapabilityViolation::UNKNOWN_TASK {
        text.push(b"there is no such task");
    } else if violation.owner == 0 {
        text.push(b"no task owns it");
    } else {
        text.push(b"task ").number(violation.owner).push(b" owns it");
    }
    text
}

fn channel_violation_text(violation: &CapabilityViolation) -> Text {
    let mut text = Text::new();
    text.push(b"channel capability violation: task ").number(violation.task);
    let transfer = matches!(
        violation.request_kind,
        Scheduler::REQUEST_CHANNEL_TRANSFER_RECEIVER | Scheduler::REQUEST_CHANNEL_TRANSFER_COMPLETION
    );
    let holder = |text: &mut Text| {
        if violation.owner == 0 {
            text.push(b"the task it was handed to has ended");
        } else if violation.owner == violation.task {
            // The task owns the capability, so the copy it used failed the
            // token check: another copy has replaced it. The owner may already
            // have closed or aborted the current copy, so the message names
            // ownership, not a live copy.
            text.push(b"this copy is stale; task ").number(violation.task).push(b" owns the capability");
        } else {
            text.push(b"task ").number(violation.owner).push(b" owns it");
        }
    };
    match violation.reason {
        CapabilityViolation::WRONG_OWNER => {
            if transfer {
                text.push(b" cannot hand the ").push(capability_name(violation.capability));
                text.push(b" of channel ").number(violation.channel).push(b" to task ");
                text.number(violation.destination).push(b": ");
            } else {
                text.push(b" cannot use the ").push(capability_name(violation.capability));
                text.push(b" of channel ").number(violation.channel).push(b" in ");
                text.push(channel_operation(violation.request_kind)).push(b": ");
            }
            holder(&mut text);
        }
        CapabilityViolation::IN_FLIGHT => {
            text.push(if transfer { b" cannot hand the " } else { b" cannot use the " });
            text.push(capability_name(violation.capability)).push(b" of channel ").number(violation.channel);
            if !transfer {
                text.push(b" in ").push(channel_operation(violation.request_kind));
            }
            text.push(b": this copy is stale; the capability was sent in a message no task has received yet");
        }
        CapabilityViolation::OVERLAPPING_OPERATION => {
            text.push(b" started a channel operation while its previous one was still pending");
        }
        CapabilityViolation::PENDING_OPERATION => {
            text.push(b" cannot hand a channel capability to task ").number(violation.destination);
            text.push(b" while an operation on it is pending");
        }
        CapabilityViolation::UNKNOWN_CHANNEL => {
            text.push(b" used channel ").number(violation.channel).push(b" after it was released");
        }
        CapabilityViolation::PUBLISHED_CAPABILITY => {
            text.push(b" cannot publish a value holding a Receiver or Completion on channel ");
            text.number(violation.channel).push(b", because every subscriber gets a copy");
        }
        CapabilityViolation::OWN_RECEIVER => {
            text.push(b" cannot send the Receiver of channel ").number(violation.channel);
            text.push(b" through that same channel, because nobody could ever receive the message");
        }
        CapabilityViolation::WRONG_KIND => {
            text.push(b" cannot use channel ").number(violation.channel).push(b" in ");
            text.push(channel_operation(violation.request_kind));
            text.push(b": that operation does not apply to this kind of channel");
        }
        _ => {
            text.push(b" released channel ").number(violation.channel).push(b" while a task still holds it");
        }
    }
    text
}

/// Keeps the defect message of a channel or task capability violation for
/// the violating task. The exclusive-move rule does not follow a capability
/// across function boundaries, so this is the check that turns two owners of
/// one Receiver or Completion into a defect.
///
/// # Safety
/// `context` is the running `Runtime` and `violation` a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_record_capability_violation_v1(
    context: *mut c_void,
    violation: *const CapabilityViolation,
) {
    // SAFETY: the caller's contract; the kernel lock serializes the reports,
    // and only this file reaches the notes.
    unsafe {
        let runtime = context.cast::<Runtime>();
        let violation = &*violation;
        let text = if violation.capability == CapabilityViolation::TASK {
            task_violation_text(violation)
        } else {
            channel_violation_text(violation)
        };
        let note = allocate(core::mem::size_of::<ViolationNote>()).cast::<ViolationNote>();
        note.write(ViolationNote {
            task: violation.task,
            message: text.into_c(),
            next: (*runtime).capability_violations.cast(),
        });
        (*runtime).capability_violations = note.cast();
    }
}

/// The capability-violation message of `task`, which the scheduler defected,
/// or null.
///
/// # Safety
/// `runtime` is the running `Runtime`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_capability_violation_of_v1(runtime: *const Runtime, task: u64) -> *const c_char {
    // SAFETY: the caller's contract; every note is one this file made.
    unsafe {
        let mut note = (*runtime).capability_violations.cast::<ViolationNote>().cast_const();
        while !note.is_null() {
            if (*note).task == task {
                return (*note).message;
            }
            note = (*note).next;
        }
        ptr::null()
    }
}

/// Builds the root's virtual-time deadlock defect: each parked task and the
/// operation it waits on.
///
/// # Safety
/// `context` is the running `Runtime`; `waiters` holds `count` records.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_record_deadlock_v1(
    context: *mut c_void,
    waiters: *const DeadlockWaiter,
    count: usize,
) {
    let mut text = Text::new();
    text.push(b"virtual-time deadlock: every task is parked and no timer is pending:");
    // SAFETY: the caller's contract.
    unsafe {
        for index in 0..count {
            let waiter = &*waiters.add(index);
            text.push(if index == 0 { b" task " } else { b"; task " }).number(waiter.task);
            text.push(b" waits on ").push(waited_operation(waiter.request_kind));
        }
        (*context.cast::<Runtime>()).deadlock_waiters = text.into_c();
    }
}

/// Keeps the latest defected incarnation and its message: the one its body
/// reported as it ended, or a stack overflow the scheduler saw.
///
/// # Safety
/// `context` is the running `Runtime` and `termination` a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_record_process_defect_v1(
    context: *mut c_void,
    termination: *const ProcessTermination,
) {
    // SAFETY: the caller's contract; the old message is one this function made.
    unsafe {
        let runtime = context.cast::<Runtime>();
        let termination = &*termination;
        let message = if termination.stack_overflow != 0 {
            let mut text = Text::new();
            text.push(b"Process stack overflow");
            text.into_c()
        } else if !termination.message.is_null() {
            let mut text = Text::new();
            text.push(CStr::from_ptr(termination.message).to_bytes());
            text.into_c()
        } else {
            ptr::null_mut()
        };
        libc::free((*runtime).last_defect_message.cast());
        (*runtime).last_defect_message = message;
        (*runtime).last_defect_incarnation = termination.incarnation;
        (*runtime).last_defect_membership = termination.membership;
        (*runtime).process_defect_count += 1;
    }
}

/// `message`, followed by the latest process defect when one happened: a
/// root that cannot finish is often waiting on a process that defected, and
/// with tracing off nothing else says so. A new message is from `malloc` and
/// belongs to the defect it is passed to.
///
/// # Safety
/// `runtime` is the running `Runtime`; `message` is a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_with_process_defect_v1(runtime: *const Runtime, message: *const c_char) -> *const c_char {
    // SAFETY: the caller's contract.
    unsafe {
        let runtime = &*runtime;
        if runtime.process_defect_count == 0 {
            return message;
        }
        let defect: &[u8] = if runtime.last_defect_message.is_null() {
            b"defect"
        } else {
            CStr::from_ptr(runtime.last_defect_message).to_bytes()
        };
        let mut text = Text::new();
        text.push(CStr::from_ptr(message).to_bytes()).push(b"; process incarnation ");
        text.number(runtime.last_defect_incarnation).push(b" of membership ");
        text.number(runtime.last_defect_membership).push(b" defected");
        if runtime.process_defect_count > 1 {
            text.push(b" (the last of ").number(runtime.process_defect_count).push(b" process defects)");
        }
        text.push(b": ").push(defect);
        text.into_c()
    }
}
