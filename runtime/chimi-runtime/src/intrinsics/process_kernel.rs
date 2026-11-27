//! The std/process and std/supervisor operations that run on the scheduler
//! (docs/architecture/supervised-process-runtime.md, "Mapping onto the
//! native runtime"), as they move from `runtime.c`. This file holds the
//! requests that carry no message: a process's own address and mailbox
//! counts, `Process.receive`, `Process.unmonitor`, and `Supervisor.address`,
//! `remove_process`, `remove_supervisor`, `stop` and `wait`.
//!
//! Only process programs link the process kernel (pay-for-play, lab
//! scenario MF06), so nothing the evaluator always links calls this file:
//! `runtime.c`'s kernel, which only the runtime variant built with
//! `CHIMI_PROCESS_KERNEL` holds, calls [`chimi_runtime_process_kernel_v1`]
//! first, and the linker drops this code from every other program.

use core::ffi::{CStr, c_char, c_void};
use core::ptr;

use super::abi::arguments;
use super::clocks::ClockCall;
use super::message::Message;
use super::standard::{defect, defect_with, shape_defect, standard_value, success, void_value};
use crate::eval::cancelled;
use crate::heap::allocator::current_carrier;
use crate::heap::fragment;
use crate::heap::layout::{
    Control, Environment, FRAGMENT_ROOTS, Intrinsic, ProcessRequest, Runtime, RuntimeFragment, Scheduler,
    StandardConstructor, Trace, Value, ValueKind,
};
use crate::heap::values::{integer_value, new_value};
use crate::scheduler::process_request;

/// A direct process address.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn direct_address(target: u64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::PROCESS_ADDRESS);
        (*value).payload.address.kind = Trace::ADDRESS_DIRECT;
        (*value).payload.address.target = target;
        value
    }
}

/// Issues `request` once; `Err` with the control to return when the fiber
/// was cancelled or the reply is not OK.
///
/// # Safety
/// Runs on a fiber of a program linked with the process kernel.
unsafe fn ask(request: &mut ProcessRequest, failed: &'static CStr, node: i64) -> Result<(), *mut Control> {
    // SAFETY: the caller's contract.
    unsafe {
        if process_request(request) {
            return Err(cancelled());
        }
        if request.status != Scheduler::RESPONSE_OK {
            return Err(defect(failed, node));
        }
    }
    Ok(())
}

/// Issues `request`, repeating it while the scheduler answers PENDING;
/// true when the fiber was cancelled.
///
/// # Safety
/// Runs on a fiber of a program linked with the process kernel.
unsafe fn ask_until_answered(request: &mut ProcessRequest) -> bool {
    loop {
        request.status = Scheduler::RESPONSE_UNSET;
        // SAFETY: the caller's contract.
        let cancel = unsafe { process_request(request) };
        if cancel || request.status != Scheduler::RESPONSE_PENDING {
            return cancel;
        }
    }
}

/// `Process.receive`: the next message, adopted into this heap; or, for a
/// monitor notice, what the watcher's own `fn(Down) -> M` makes of the
/// `Down` value.
///
/// # Safety
/// Runs on a process body's fiber of a program linked with the process
/// kernel.
unsafe fn receive(call: ClockCall) -> *mut Control {
    // SAFETY: the caller's contract; a SUCCEEDED reply hands this fiber the
    // message fragment.
    unsafe {
        let node = i64::from(call.call_node);
        let mut request = ProcessRequest::of(Scheduler::REQUEST_PROCESS_RECEIVE);
        if ask_until_answered(&mut request) {
            if request.status == Scheduler::RESPONSE_SUCCEEDED {
                fragment::free(current_carrier(), request.message as usize as *mut RuntimeFragment);
            }
            return cancelled();
        }
        match request.status {
            Scheduler::RESPONSE_SUCCEEDED => {
                let mut roots = [ptr::null_mut::<c_void>(); FRAGMENT_ROOTS];
                let taken = request.message as usize as *mut RuntimeFragment;
                fragment::adopt(current_carrier(), taken, roots.as_mut_ptr());
                let message = roots[0].cast::<Value>();
                if request.down == 0 {
                    return success(message);
                }
                if request.down > Scheduler::DOWN_UNAVAILABLE || (*message).kind != ValueKind::CALLABLE {
                    return defect(c"Process.receive scheduler request failed", node);
                }
                let down = standard_value(
                    StandardConstructor::DOWN_RETURNED + (request.down - Scheduler::DOWN_RETURNED),
                    &[],
                );
                call.call((*message).payload.callable, &mut [down])
            }
            Scheduler::RESPONSE_CANCELLED | Scheduler::RESPONSE_FORCED => cancelled(),
            _ => defect(c"Process.receive scheduler request failed", node),
        }
    }
}

/// A removal that the scheduler refused: the handle belongs to another
/// supervisor (`owner`), or names no current member.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn removal_defect(function: &[u8], owner: u64, supervisor: u64, node: i64) -> *mut Control {
    let message = Message::new().text(function);
    let message: *const c_char = if owner != 0 {
        message
            .text(b": the handle belongs to supervisor ")
            .unsigned(owner)
            .text(b", not supervisor ")
            .unsigned(supervisor)
            .finish()
    } else {
        message
            .text(b": the handle names no current member of supervisor ")
            .unsigned(supervisor)
            .text(
                b"; it was removed already, through this handle or a copy of it, or the supervisor has stopped",
            )
            .finish()
    };
    // SAFETY: the caller's contract.
    unsafe { defect_with(message, node) }
}

/// The intrinsic `intrinsic` of the process kernel on `arguments`, or null
/// when `runtime.c` still serves it.
///
/// # Safety
/// Runs on a fiber of a program linked with the process kernel; the
/// arguments are live and the call's fields are its own.
unsafe fn process_kernel_operation(call: ClockCall, intrinsic: u32, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call.call_node);
        let shape = |kinds: &[ValueKind]| {
            arguments.len() == kinds.len() && arguments.iter().zip(kinds).all(|(&value, &kind)| (*value).kind == kind)
        };
        let managed = |index: usize| (*arguments[index]).payload.managed;
        match intrinsic {
            Intrinsic::PROCESS_OWN_ADDRESS => {
                if !shape(&[ValueKind::PROCESS_INBOX]) {
                    return shape_defect(call.call_node);
                }
                let mut request = ProcessRequest::of(Scheduler::REQUEST_PROCESS_SELF);
                match ask(&mut request, c"Process.address scheduler request failed", node) {
                    Ok(()) => success(direct_address(request.address)),
                    Err(stopped) => stopped,
                }
            }
            Intrinsic::PROCESS_QUEUED | Intrinsic::PROCESS_CAPACITY => {
                if !shape(&[ValueKind::PROCESS_INBOX]) {
                    return shape_defect(call.call_node);
                }
                let mut request = ProcessRequest::of(Scheduler::REQUEST_PROCESS_MAILBOX);
                if let Err(stopped) = ask(&mut request, c"Process mailbox scheduler request failed", node) {
                    return stopped;
                }
                let count = if intrinsic == Intrinsic::PROCESS_QUEUED {
                    request.queued as i64
                } else {
                    i64::from(request.capacity)
                };
                success(integer_value(ValueKind::INTEGER, count))
            }
            Intrinsic::PROCESS_UNMONITOR => {
                if !shape(&[ValueKind::PROCESS_INBOX, ValueKind::PROCESS_MONITOR]) {
                    return shape_defect(call.call_node);
                }
                let mut request = ProcessRequest::of(Scheduler::REQUEST_PROCESS_UNMONITOR);
                request.monitor = (*arguments[1]).payload.address.target;
                match ask(&mut request, c"Process.unmonitor scheduler request failed", node) {
                    Ok(()) => success(void_value()),
                    Err(stopped) => stopped,
                }
            }
            Intrinsic::PROCESS_RECEIVE => {
                if !shape(&[ValueKind::PROCESS_INBOX]) {
                    return shape_defect(call.call_node);
                }
                receive(call)
            }
            Intrinsic::SUPERVISOR_ADDRESS => {
                if !shape(&[ValueKind::MANAGED_PROCESS]) {
                    return shape_defect(call.call_node);
                }
                let mut request = ProcessRequest::of(Scheduler::REQUEST_SUPERVISOR_ADDRESS);
                request.supervisor = managed(0).supervisor;
                request.membership = managed(0).membership;
                match ask(&mut request, c"Supervisor.address scheduler request failed", node) {
                    Ok(()) => success(direct_address(request.address)),
                    Err(stopped) => stopped,
                }
            }
            Intrinsic::SUPERVISOR_REMOVE_PROCESS | Intrinsic::SUPERVISOR_REMOVE_SUPERVISOR => {
                let process = intrinsic == Intrinsic::SUPERVISOR_REMOVE_PROCESS;
                let (function, member, kind): (&[u8], _, _) = if process {
                    (
                        b"Supervisor.remove_process",
                        ValueKind::MANAGED_PROCESS,
                        Scheduler::REQUEST_SUPERVISOR_REMOVE_PROCESS,
                    )
                } else {
                    (
                        b"Supervisor.remove_supervisor",
                        ValueKind::MANAGED_SUPERVISOR,
                        Scheduler::REQUEST_SUPERVISOR_REMOVE_SUPERVISOR,
                    )
                };
                if !shape(&[ValueKind::SUPERVISOR_RUNNING, member]) {
                    return shape_defect(call.call_node);
                }
                let supervisor = managed(0).supervisor;
                let mut request = ProcessRequest::of(kind);
                request.supervisor = supervisor;
                request.membership = managed(1).membership;
                if ask_until_answered(&mut request) {
                    return cancelled();
                }
                if request.status == Scheduler::RESPONSE_OK {
                    return success(void_value());
                }
                removal_defect(function, request.owner, supervisor, node)
            }
            Intrinsic::SUPERVISOR_STOP | Intrinsic::SUPERVISOR_WAIT => {
                let stop = intrinsic == Intrinsic::SUPERVISOR_STOP;
                if !shape(&[ValueKind::SUPERVISOR_RUNNING]) {
                    return shape_defect(call.call_node);
                }
                let kind = if stop { Scheduler::REQUEST_SUPERVISOR_STOP } else { Scheduler::REQUEST_SUPERVISOR_WAIT };
                let mut request = ProcessRequest::of(kind);
                request.supervisor = managed(0).supervisor;
                if ask_until_answered(&mut request) {
                    return cancelled();
                }
                if request.status == Scheduler::RESPONSE_OK {
                    return success(void_value());
                }
                // The supervisor escalated: the program ends with the
                // escalation's defect.
                if !stop && request.status == Scheduler::RESPONSE_DEFECTED {
                    return defect(c"Supervisor.RestartLimitExceeded", node);
                }
                if stop {
                    defect(c"Supervisor.stop scheduler request failed", node)
                } else {
                    defect(c"Supervisor.wait scheduler request failed", node)
                }
            }
            _ => ptr::null_mut(),
        }
    }
}

/// [`process_kernel_operation`] for `runtime.c`'s kernel: null when the
/// intrinsic is still served there.
///
/// # Safety
/// As for [`process_kernel_operation`]; `values` holds `count` live values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_process_kernel_v1(
    runtime: *mut Runtime,
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    span: u64,
    call_node: u32,
    environment: *mut Environment,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let call = ClockCall { runtime, span, call_node, environment };
        process_kernel_operation(call, intrinsic, arguments(values, count))
    }
}
