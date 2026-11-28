//! The fiber entries of tasks and process incarnations, their result words,
//! and the scheduler's release hooks for the words and contexts it holds
//! (CHI-305 step 5, phase 8).
//!
//! A child task's result outlives its heap as a fragment. The scheduler
//! keeps it with the task record, and every `Task.result` copies it into the
//! caller's heap. The scheduler can also hand a child's defect code to the
//! root, so a child's result word carries [`CHILD_RESULT_TAG`] to tell it
//! from the root's own `Control` pointer.
//!
//! The release hooks run on the host stack with no task running, so they
//! free fragments without touching the fiber context.

use core::ffi::{c_char, c_void};
use core::ptr;

use super::capabilities::chimi_runtime_transfer_to_child_v1;
use super::standard::defect;
use crate::eval::{FOREIGN_POINTER_DEFECT, cancelled, chimi_runtime_invoke_v1, take_spans};
use crate::heap::allocator::{carrier_count, current_carrier};
use crate::heap::fragment;
use crate::heap::layout::{
    AllocationKind, CHILD_RESULT_TAG, Control, ControlKind, Environment, FRAGMENT_ROOTS, FiberResult, HeapStatus, ProcessRequest,
    ProcessSpecContext, Runtime, RuntimeFragment, RuntimeHeap, Scheduler, TaskContext, Value, ValueKind,
};
use crate::heap::values::{new_environment, new_value};
use crate::scheduler;
use crate::telemetry::chimi_trace_process_body_v1;


/// `ChimiNodeFn`: the compiled root entry the root task runs.
type NodeFn = unsafe extern "C" fn(*mut Runtime, *mut Environment, u64) -> *mut Control;

unsafe extern "C" {
    /// The heap the root task runs in (runtime.c).
    static mut chimi_runtime_main_heap: RuntimeHeap;
    /// The incarnation the running process body belongs to (chimi_fiber.h).
    fn chimi_fiber_current_incarnation() -> u64;
    /// Records why the running process body defected (chimi_fiber.h).
    fn chimi_fiber_report_defect(message: *const c_char);
}

/// The result of a fiber whose outcome is `control`, which lives on.
///
/// # Safety
/// `control` is live.
unsafe fn task_fiber_result(control: *mut Control) -> FiberResult {
    // SAFETY: the caller's contract.
    let kind = unsafe { (*control).kind };
    let word = control as i64;
    match kind {
        ControlKind::CANCEL => FiberResult { state: FiberResult::CANCELLED, value: 0, defect_code: 0 },
        ControlKind::DEFECT => FiberResult { state: FiberResult::DEFECTED, value: 0, defect_code: word },
        _ => FiberResult { state: FiberResult::SUCCEEDED, value: word, defect_code: 0 },
    }
}

/// A child's result: `control` copied into a fragment, as a tagged word.
///
/// # Safety
/// Runs on the child's fiber; `control` is live in its heap.
unsafe fn child_task_result(control: *mut Control) -> FiberResult {
    // SAFETY: the caller's contract.
    unsafe {
        if (*control).kind == ControlKind::CANCEL {
            return task_fiber_result(control);
        }
        let mut control = control;
        let kinds = [AllocationKind::CONTROL];
        let mut result: *mut RuntimeFragment = ptr::null_mut();
        let mut roots = [control.cast::<c_void>()];
        if fragment::copy(current_carrier(), roots.as_ptr(), kinds.as_ptr(), 1, &mut result) != HeapStatus::OK {
            control = defect(FOREIGN_POINTER_DEFECT, -1);
            roots[0] = control.cast();
            if fragment::copy(current_carrier(), roots.as_ptr(), kinds.as_ptr(), 1, &mut result) != HeapStatus::OK {
                libc::abort();
            }
        }
        let word = result as i64 | CHILD_RESULT_TAG;
        if (*control).kind == ControlKind::DEFECT {
            FiberResult { state: FiberResult::DEFECTED, value: 0, defect_code: word }
        } else {
            FiberResult { state: FiberResult::SUCCEEDED, value: word, defect_code: 0 }
        }
    }
}

/// Aborts unless this thread entered its own carrier. With one carrier every
/// thread may use carrier 0; with several, a thread that never entered its
/// carrier would share carrier 0's scratch. `defect` names the fiber started.
fn require_bound_carrier(defect: &[u8]) {
    if carrier_count() <= 1 {
        return;
    }
    let carrier = current_carrier();
    // SAFETY: the running carrier is this thread's; `thread` holds a
    // pthread_t once `bound` is set.
    let bound = unsafe {
        (*carrier).bound && libc::pthread_equal((*carrier).thread as libc::pthread_t, libc::pthread_self()) != 0
    };
    if !bound {
        // SAFETY: a plain write of a static message, then abort.
        unsafe {
            libc::write(2, defect.as_ptr().cast(), defect.len());
            libc::abort();
        }
    }
}

const UNBOUND_TASK: &[u8] = b"Chimi runtime defect: a task started on a thread without its own carrier\n";
const UNBOUND_PROCESS: &[u8] = b"Chimi runtime defect: a process started on a thread without its own carrier\n";

/// The fiber entry of a task: the root runs its compiled entry in the main
/// heap; a child adopts its captures in a heap of its own, calls its
/// callable, hands its result's exclusive capabilities to its parent, and
/// ends with its result copied out. Task.start (intrinsics/tasks.rs) passes
/// it with the malloc'd context, which a child frees.
///
/// # Safety
/// Runs as a fiber entry; `raw_context` is a live `TaskContext`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_task_entry_v1(raw_context: *mut c_void) -> FiberResult {
    let context = raw_context.cast::<TaskContext>();
    require_bound_carrier(UNBOUND_TASK);
    // SAFETY: the caller's contract; the carrier is re-read after each call
    // that may park, since the fiber may resume on another carrier.
    unsafe {
        (*current_carrier()).fiber.target = ptr::null_mut();
        if !(*context).entry.is_null() {
            (*current_carrier()).fiber.heap = &raw mut chimi_runtime_main_heap;
            let entry = core::mem::transmute::<*const c_void, NodeFn>((*context).entry);
            return task_fiber_result(entry((*context).runtime, (*context).environment, (*context).span));
        }
        let runtime = (*context).runtime;
        let span = (*context).span;
        let heap = fragment::heap_create(current_carrier());
        (*current_carrier()).fiber.heap = heap;
        let parent = (*context).parent;
        let mut captures = [ptr::null_mut::<c_void>(); FRAGMENT_ROOTS];
        fragment::adopt(current_carrier(), (*context).captures, captures.as_mut_ptr());
        libc::free(context.cast());
        let callable = captures[0].cast::<Value>();
        let mut control = chimi_runtime_invoke_v1(
            runtime,
            (*callable).payload.callable,
            ptr::null_mut(),
            0,
            span,
            u32::MAX,
            captures[1].cast(),
        );
        // Only the parent can read the result (a task handle is
        // parent-local), so the result's exclusive capabilities become the
        // parent's before this task ends and its cleanup would abort them.
        let kind = (*control).kind;
        if (kind == ControlKind::SUCCESS || kind == ControlKind::FAILURE) && !(*control).value.is_null() {
            let mut cancelled_meanwhile = false;
            if !chimi_runtime_transfer_to_child_v1((*control).value, parent, &mut cancelled_meanwhile) {
                control = if cancelled_meanwhile {
                    cancelled()
                } else {
                    defect(c"Task could not hand an exclusive capability in its result to its parent", -1)
                };
            }
        }
        let result = child_task_result(control);
        fragment::heap_destroy(current_carrier(), heap);
        (*current_carrier()).fiber.heap = ptr::null_mut();
        result
    }
}

/// The control a task's result word names: the root's own control, or a
/// copy of a child's result fragment in the current heap.
///
/// # Safety
/// Runs on a fiber that may allocate; `raw` is a live task result word.
pub(crate) unsafe fn task_reply(raw: i64) -> *mut Control {
    if raw & CHILD_RESULT_TAG == 0 {
        return raw as *mut Control;
    }
    let mut roots = [ptr::null_mut::<c_void>(); FRAGMENT_ROOTS];
    // SAFETY: the caller's contract; an untagged word is a live fragment.
    unsafe {
        fragment::copy_into_current(
            current_carrier(),
            (raw & !CHILD_RESULT_TAG) as *const RuntimeFragment,
            roots.as_mut_ptr(),
        );
    }
    roots[0].cast()
}

/// [`task_reply`] for runtime.c's root outcome.
///
/// # Safety
/// As for [`task_reply`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_task_reply_v1(raw: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { task_reply(raw) }
}

/// Frees a task word: a child's tagged result fragment. The root's own
/// control lives in the main heap and is not the scheduler's to free.
///
/// # Safety
/// `word` is a task word the scheduler no longer holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_release_task_word_v1(_context: *mut c_void, word: i64) {
    if word & CHILD_RESULT_TAG != 0 {
        // SAFETY: the caller's contract.
        unsafe { fragment::free(current_carrier(), (word & !CHILD_RESULT_TAG) as *mut RuntimeFragment) };
    }
}

/// Frees a channel item's fragment.
///
/// # Safety
/// `word` is a live fragment the scheduler no longer holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_release_channel_word_v1(_context: *mut c_void, word: i64) {
    // SAFETY: the caller's contract.
    unsafe { fragment::free(current_carrier(), word as *mut RuntimeFragment) };
}

/// A copy of a channel item's fragment, for a broadcast or watch.
///
/// # Safety
/// `word` is a live fragment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_copy_channel_word_v1(_context: *mut c_void, word: i64) -> i64 {
    // SAFETY: the caller's contract.
    unsafe { fragment::duplicate(current_carrier(), word as *const RuntimeFragment) as i64 }
}

/// Frees the context of a child the scheduler ended before its entry ran,
/// which never adopted its captures.
///
/// # Safety
/// `raw` is a live, unadopted malloc'd `TaskContext`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_release_task_context_v1(_context: *mut c_void, raw: *mut c_void) {
    let task = raw.cast::<TaskContext>();
    // SAFETY: the caller's contract.
    unsafe {
        fragment::free(current_carrier(), (*task).captures);
        libc::free(task.cast());
    }
}

/// Frees a process spec's context once the scheduler drops it.
///
/// # Safety
/// `raw` is a live malloc'd `ProcessSpecContext`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_release_spec_context_v1(_context: *mut c_void, raw: *mut c_void) {
    let spec = raw.cast::<ProcessSpecContext>();
    // SAFETY: the caller's contract.
    unsafe {
        fragment::free(current_carrier(), (*spec).body);
        libc::free(spec.cast());
    }
}

/// Frees a message the scheduler drops undelivered.
///
/// # Safety
/// `message` is a live fragment the scheduler no longer holds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_release_message_v1(_context: *mut c_void, message: u64) {
    // SAFETY: the caller's contract.
    unsafe { fragment::free(current_carrier(), message as usize as *mut RuntimeFragment) };
}

/// The body of every incarnation: runs a copy of the spec's body, with the
/// start argument when it was handed off, in a heap of its own, and ends
/// with a `CHIMI_PROCESS_END_*` word. runtime.c's `chimi_rt_process_entry`,
/// which only the process kernel's variant defines, calls it.
///
/// # Safety
/// Runs as a fiber entry; `raw_context` is a live `ProcessSpecContext`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_process_entry_v1(raw_context: *mut c_void) -> FiberResult {
    let context = raw_context.cast::<ProcessSpecContext>();
    require_bound_carrier(UNBOUND_PROCESS);
    // SAFETY: the caller's contract; the carrier is re-read after each call
    // that may park.
    unsafe {
        (*current_carrier()).fiber.target = ptr::null_mut();
        let runtime = (*context).runtime;
        let heap = fragment::heap_create(current_carrier());
        (*current_carrier()).fiber.heap = heap;
        let mut roots = [ptr::null_mut::<c_void>(); FRAGMENT_ROOTS];
        fragment::copy_into_current(current_carrier(), (*context).body, roots.as_mut_ptr());
        let body = roots[0].cast::<Value>();
        let mut arguments = [new_value(ValueKind::PROCESS_INBOX), ptr::null_mut()];
        let mut argument_count = 1;
        let mut control: *mut Control = ptr::null_mut();
        if (*context).handoff {
            // The start argument travelled as a message: adopting it makes
            // this heap its owner, and the reply moved its capabilities to
            // this incarnation.
            let mut request: ProcessRequest = core::mem::zeroed();
            request.kind = Scheduler::REQUEST_PROCESS_ARGUMENT;
            if scheduler::process_request(&mut request) {
                if request.status == Scheduler::RESPONSE_OK {
                    fragment::free(current_carrier(), request.message as usize as *mut RuntimeFragment);
                }
                control = cancelled();
            } else if request.status != Scheduler::RESPONSE_OK {
                control = defect(c"Supervisor.start_handoff: the incarnation has no start argument", u32::MAX as i64);
            } else {
                let mut argument_roots = [ptr::null_mut::<c_void>(); FRAGMENT_ROOTS];
                fragment::adopt(
                    current_carrier(),
                    request.message as usize as *mut RuntimeFragment,
                    argument_roots.as_mut_ptr(),
                );
                arguments[argument_count] = argument_roots[0].cast();
                argument_count += 1;
            }
        }
        let span = take_spans(runtime, 1);
        let traced = !(*runtime).silent && control.is_null();
        let incarnation = if traced { chimi_fiber_current_incarnation() } else { 0 };
        if traced {
            chimi_trace_process_body_v1(runtime, span, incarnation, ptr::null());
        }
        if control.is_null() {
            control = chimi_runtime_invoke_v1(
                runtime,
                (*body).payload.callable,
                arguments.as_mut_ptr(),
                argument_count as u32,
                span,
                u32::MAX,
                new_environment(0),
            );
        }
        if traced {
            chimi_trace_process_body_v1(runtime, span, incarnation, control);
        }
        let result = match (*control).kind {
            ControlKind::SUCCESS => {
                FiberResult { state: FiberResult::SUCCEEDED, value: FiberResult::PROCESS_END_RETURNED, defect_code: 0 }
            }
            ControlKind::FAILURE => {
                FiberResult { state: FiberResult::SUCCEEDED, value: FiberResult::PROCESS_END_FAILED, defect_code: 0 }
            }
            ControlKind::CANCEL => FiberResult { state: FiberResult::CANCELLED, value: 0, defect_code: 0 },
            _ => {
                chimi_fiber_report_defect((*control).message);
                FiberResult { state: FiberResult::DEFECTED, value: 0, defect_code: 1 }
            }
        };
        let carrier = current_carrier();
        if (*heap).peak_live_bytes > (*carrier).process_heap_peak_value_bytes {
            (*carrier).process_heap_peak_value_bytes = (*heap).peak_live_bytes;
        }
        fragment::heap_destroy(carrier, heap);
        (*carrier).fiber.heap = ptr::null_mut();
        result
    }
}
