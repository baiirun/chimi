//! The intrinsics that run a callback inside a scope: `Cause.context`,
//! which adds a frame to the cause of what the callback produced;
//! `Test.capture_defect`, which turns the callback's defect into its
//! message; and `Resource.scoped`, which runs acquire, use and release with
//! the scheduler masking cancellation around acquire and release. Each
//! moved from `runtime.c` once the evaluator could call back from Rust.

use core::ptr;

use super::causes::{cause_with_frame, control_cause, control_with_cause};
use super::debug::render;
use super::standard::{defect, defect_with, shape_defect, success};
use super::text::HeapText;
use crate::eval::{cancelled, invoke};
use crate::heap::layout::{Control, ControlKind, Environment, Runtime, Scheduler, SchedulerRequest, Value, ValueKind};
use crate::heap::values::text_value;
use crate::scheduler::{self, cancel_is_terminal, requires_scheduler};

unsafe extern "C" {
    /// The call nodes of std/test's assertions, ascending (program data).
    static CHIMI_ASSERTION_NODES: [u32; 0];
    static CHIMI_ASSERTION_NODE_COUNT: u32;
}

/// The call a scope intrinsic runs its callbacks under.
#[derive(Clone, Copy)]
pub struct ScopeCall {
    pub runtime: *mut Runtime,
    pub span: u64,
    pub call_node: u32,
    pub environment: *mut Environment,
}

impl ScopeCall {
    /// Calls the callable `function` with `arguments`.
    ///
    /// # Safety
    /// Runs on a fiber; `function` is a live Callable value.
    unsafe fn run(self, function: *mut Value, arguments: &mut [*mut Value]) -> *mut Control {
        // SAFETY: the caller's contract.
        unsafe {
            let values = if arguments.is_empty() { ptr::null_mut() } else { arguments.as_mut_ptr() };
            invoke(
                self.runtime,
                (*function).payload.callable,
                values,
                arguments.len() as u32,
                self.span,
                self.call_node,
                self.environment,
            )
        }
    }
}

/// # Safety
/// `value` is live.
unsafe fn is(value: *mut Value, kind: ValueKind) -> bool {
    // SAFETY: the caller's contract.
    unsafe { (*value).kind == kind }
}

/// `Cause.context(operation, text)`: runs `operation` and adds a frame
/// naming `text` and the call to the cause of what it produced.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn cause_context(call: ScopeCall, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if arguments.len() != 2 || !is(arguments[0], ValueKind::CALLABLE) || !is(arguments[1], ValueKind::TEXT) {
            return shape_defect(call.call_node);
        }
        let control = call.run(arguments[0], &mut []);
        let cause = control_cause(control);
        if cause.is_null() {
            return control;
        }
        control_with_cause(control, cause_with_frame(cause, arguments[1], i64::from(call.call_node)))
    }
}

/// Whether `node` is the call node of one of std/test's assertions.
fn is_assertion_call(node: i64) -> bool {
    // SAFETY: program data, ascending, with its count.
    let nodes = unsafe {
        core::slice::from_raw_parts((&raw const CHIMI_ASSERTION_NODES).cast::<u32>(), CHIMI_ASSERTION_NODE_COUNT as usize)
    };
    u32::try_from(node).is_ok_and(|node| nodes.binary_search(&node).is_ok())
}

/// A defect whose message is `heading`, a line break, and `value` in the
/// indented debug format.
///
/// # Safety
/// Runs on a fiber that may allocate; `value` is live.
unsafe fn capture_outcome_defect(heading: &[u8], value: *const Value, call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let mut message = HeapText::new();
        message.push_bytes(heading);
        message.push_bytes(b"\n");
        let rendering = render(value, true);
        let text = (*rendering).payload.text;
        message.push_bytes(core::slice::from_raw_parts(text.bytes.cast::<u8>(), text.length));
        message.push_bytes(b"\0");
        defect_with(message.data().cast(), i64::from(call_node))
    }
}

/// A scope request of `kind` with `token`, answered in place; true when
/// the fiber resumed to be cancelled.
///
/// # Safety
/// As for [`scheduler::request`].
unsafe fn scope_request(kind: u32, request: &mut SchedulerRequest) -> bool {
    request.kind = kind;
    // SAFETY: the caller's contract.
    unsafe { scheduler::request(request) }
}

/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn capture_failed(call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { defect(c"Test.capture_defect scheduler request failed", i64::from(call_node)) }
}

/// `Test.capture_defect(operation)`, as `invoke_capture_defect` in
/// execution/core_operations.rs: returns the message of the defect
/// `operation` raises. An assertion's defect is not captured, so a failing
/// assertion still fails its test. Under the scheduler the operation runs in
/// a scope without a deadline; after a captured defect, the scope's children
/// are cancelled and leaving it drains them. A cancellation from outside
/// passes through, as through `Task.timeout`.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn capture_defect(call: ScopeCall, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let call_node = call.call_node;
        if arguments.len() != 1 || !is(arguments[0], ValueKind::CALLABLE) {
            return shape_defect(call_node);
        }
        let scheduled = requires_scheduler();
        let mut enter = SchedulerRequest::of(Scheduler::REQUEST_SCOPE_ENTER);
        enter.value = i64::MAX;
        if scheduled {
            if scheduler::request(&mut enter) {
                return cancelled();
            }
            if enter.status != Scheduler::RESPONSE_OK {
                return capture_failed(call_node);
            }
        }
        let control = call.run(arguments[0], &mut []);
        if (*control).kind == ControlKind::CANCEL && cancel_is_terminal() {
            return control;
        }
        let captured = (*control).kind == ControlKind::DEFECT && !is_assertion_call((*control).node);
        if scheduled {
            if captured {
                let mut cancel = SchedulerRequest::of(Scheduler::REQUEST_SCOPE_CANCEL);
                cancel.token = enter.token;
                if scheduler::request(&mut cancel) {
                    return cancelled();
                }
                if cancel.status != Scheduler::RESPONSE_OK {
                    return capture_failed(call_node);
                }
            }
            loop {
                // A captured defect discards the operation's values, so the
                // capabilities it gained in the scope are aborted, as after
                // a cancellation.
                let mut exit = SchedulerRequest::of(Scheduler::REQUEST_SCOPE_EXIT);
                exit.token = enter.token;
                exit.value = i64::from(captured);
                if scheduler::request(&mut exit) {
                    return cancelled();
                }
                if exit.status == Scheduler::RESPONSE_PENDING {
                    continue;
                }
                if exit.status != Scheduler::RESPONSE_OK {
                    return capture_failed(call_node);
                }
                break;
            }
        }
        if captured {
            let message = (*control).message;
            return success(text_value(message, libc::strlen(message)));
        }
        match (*control).kind {
            ControlKind::SUCCESS => capture_outcome_defect(
                b"Test.capture_defect: the operation returned instead of defecting:",
                (*control).value,
                call_node,
            ),
            ControlKind::FAILURE => capture_outcome_defect(
                b"Test.capture_defect: the operation failed with an expected failure instead of defecting:",
                (*control).value,
                call_node,
            ),
            _ => control,
        }
    }
}

/// The outcome of `Resource.scoped` once release has run
/// (std/resource.chimi): a use defect stays; then a release defect; then a
/// use cancellation; then a release failure or cancellation replaces the
/// use's value or failure.
///
/// # Safety
/// Both are live.
unsafe fn after_release(used: *mut Control, released: *mut Control) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if (*used).kind == ControlKind::DEFECT {
            return used;
        }
        if (*released).kind == ControlKind::DEFECT {
            return released;
        }
        if (*used).kind == ControlKind::CANCEL {
            return used;
        }
        if matches!((*released).kind, ControlKind::FAILURE | ControlKind::CANCEL) {
            return released;
        }
        used
    }
}

/// `outcome` when it is a defect, else a cancellation.
///
/// # Safety
/// Runs on a fiber that may allocate; `outcome` is live.
unsafe fn cancelled_unless_defect(outcome: *mut Control) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { if (*outcome).kind == ControlKind::DEFECT { outcome } else { cancelled() } }
}

/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn resource_failed(call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { defect(c"Resource.scoped scheduler request failed", i64::from(call_node)) }
}

/// `Resource.scoped(acquire, release, use)` (std/resource.chimi). Under the
/// scheduler the call is an entry on the task's scope stack
/// (scheduler/scope.rs): entered masked before acquire, so a cancellation of
/// the task always finds it and none lands between acquire returning and
/// the entry owning its value; unmasked for use, which delivers a
/// cancellation acquire held off; masked again before release, so no
/// cancellation interrupts release; left after it, which delivers a
/// cancellation release held off. A forced end (a terminal cancel) issues no
/// request and runs no release it has not begun: the fiber must not park.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn resource_scoped(call: ScopeCall, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let call_node = call.call_node;
        if arguments.len() != 3 || !arguments.iter().all(|&argument| is(argument, ValueKind::CALLABLE)) {
            return shape_defect(call_node);
        }
        let (acquire, release, use_) = (arguments[0], arguments[1], arguments[2]);
        let scheduled = requires_scheduler();
        let mut entry = SchedulerRequest::of(0);
        if scheduled {
            let cancel = scope_request(Scheduler::REQUEST_RESOURCE_ENTER, &mut entry);
            if entry.status == Scheduler::RESPONSE_UNSET {
                return cancelled();
            }
            if entry.status != Scheduler::RESPONSE_OK {
                return resource_failed(call_node);
            }
            if cancel {
                // Nothing is acquired yet: leave the entry and pass the
                // cancellation on.
                if !cancel_is_terminal() {
                    let mut exit = SchedulerRequest::of(0);
                    exit.token = entry.token;
                    scope_request(Scheduler::REQUEST_RESOURCE_EXIT, &mut exit);
                }
                return cancelled();
            }
        }
        let token = entry.token;
        let with_token = || {
            let mut request = SchedulerRequest::of(0);
            request.token = token;
            request
        };
        let acquired = call.run(acquire, &mut []);
        if (*acquired).kind != ControlKind::SUCCESS {
            if scheduled && !((*acquired).kind == ControlKind::CANCEL && cancel_is_terminal()) {
                let mut exit = with_token();
                let cancel = scope_request(Scheduler::REQUEST_RESOURCE_EXIT, &mut exit);
                if cancel || exit.value == 1 {
                    return cancelled_unless_defect(acquired);
                }
                if exit.status != Scheduler::RESPONSE_OK {
                    return resource_failed(call_node);
                }
            }
            return acquired;
        }
        let handle = (*acquired).value;
        let mut used = ptr::null_mut();
        if scheduled {
            // Acquire ran masked; unmask for use. A cancellation acquire held
            // off is delivered here, and release then runs without use.
            let mut unmask = with_token();
            let cancel = scope_request(Scheduler::REQUEST_RESOURCE_USE, &mut unmask);
            if cancel && cancel_is_terminal() {
                return cancelled();
            }
            if unmask.status == Scheduler::RESPONSE_UNSET {
                return cancelled();
            }
            if unmask.status != Scheduler::RESPONSE_OK {
                return resource_failed(call_node);
            }
            if cancel || unmask.value == 1 {
                used = cancelled();
            }
        }
        if used.is_null() {
            used = call.run(use_, &mut [handle]);
        }
        if (*used).kind == ControlKind::CANCEL && cancel_is_terminal() {
            return used;
        }
        if scheduled {
            let mut mask = with_token();
            let cancel = scope_request(Scheduler::REQUEST_RESOURCE_RELEASE, &mut mask);
            if cancel && cancel_is_terminal() {
                return cancelled_unless_defect(used);
            }
            if mask.status != Scheduler::RESPONSE_OK {
                if mask.status == Scheduler::RESPONSE_UNSET {
                    return cancelled_unless_defect(used);
                }
                return resource_failed(call_node);
            }
        }
        let released = call.run(release, &mut [handle]);
        let outcome = after_release(used, released);
        if (*released).kind == ControlKind::CANCEL && cancel_is_terminal() {
            return outcome;
        }
        if scheduled {
            let mut exit = with_token();
            let cancel = scope_request(Scheduler::REQUEST_RESOURCE_EXIT, &mut exit);
            if cancel || exit.value == 1 {
                return cancelled_unless_defect(outcome);
            }
            if exit.status != Scheduler::RESPONSE_OK {
                return resource_failed(call_node);
            }
        }
        outcome
    }
}
