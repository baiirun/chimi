//! `Task.start` and the combinators `Task.all`, `Task.results` and
//! `Task.race` (time-and-outcomes.md, "Task combinators"). Moved from
//! `runtime.c` once Rust could make scheduler requests (`crate::scheduler`).
//!
//! A started task runs in its own heap, so it gets a copy of what it
//! captured: a two-root fragment of the callable and the caller's
//! requirements, which the fiber entry (`task_entry.rs`) adopts.

use core::ffi::c_void;
use core::ptr;

use super::capabilities::{self, chimi_runtime_transfer_to_child_v1};
use super::causes::{cause_with_sibling, control_cause, control_with_cause, with_stored_cause};
use super::standard::{defect, shape_defect, standard_value, success};
use super::task_entry::{chimi_runtime_task_entry_v1, task_reply};
use crate::eval::{FOREIGN_POINTER_DEFECT, cancelled};
use crate::heap::allocator::current_carrier;
use crate::heap::fragment;
use crate::heap::layout::{
    AllocationKind, Control, ControlKind, Environment, HeapStatus, Intrinsic, Runtime, RuntimeFragment, Scheduler,
    SchedulerRequest, StandardConstructor, TaskContext, Value, ValueKind,
};
use crate::heap::list;
use crate::heap::values::{capture_requirements, new_value_slots};
use crate::scheduler::{self, cancel_is_terminal};

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// A Task handle value, counted as a live reference.
///
/// # Safety
/// Runs on a fiber that may allocate; `raw` names a task.
pub(crate) unsafe fn task_value(raw: u64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { capabilities::task_value(raw) }
}

/// The call a task operation belongs to.
#[derive(Clone, Copy)]
pub struct TaskCall {
    pub runtime: *mut Runtime,
    pub span: u64,
    pub call_node: u32,
    pub environment: *mut Environment,
}

/// `Task.start`, `Task.all`, `Task.results` or `Task.race` on `arguments`.
///
/// # Safety
/// Runs on a fiber of a scheduled program that may allocate; `arguments`
/// are live and the call's fields are its own.
pub unsafe fn task_operation(call: TaskCall, intrinsic: u32, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let one = |kind: ValueKind| arguments.len() == 1 && (*arguments[0]).kind == kind;
        match intrinsic {
            Intrinsic::TASK_START if one(ValueKind::CALLABLE) => start(call, arguments[0]),
            Intrinsic::TASK_ALL | Intrinsic::TASK_RESULTS | Intrinsic::TASK_RACE if one(ValueKind::LIST) => {
                combine(call, intrinsic, arguments[0])
            }
            _ => shape_defect(call.call_node),
        }
    }
}

/// Starts `callable` as a child of this task and gives its handle.
///
/// # Safety
/// As for [`task_operation`]; `callable` is a Callable.
pub(crate) unsafe fn start(call: TaskCall, callable: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = i64::from(call.call_node);
        let roots = [callable.cast::<c_void>(), capture_requirements(call.environment).cast::<c_void>()];
        let kinds = [AllocationKind::VALUE, AllocationKind::ENVIRONMENT];
        let mut captures: *mut RuntimeFragment = ptr::null_mut();
        if fragment::copy(current_carrier(), roots.as_ptr(), kinds.as_ptr(), 2, &raw mut captures) != HeapStatus::OK {
            return defect(FOREIGN_POINTER_DEFECT, node);
        }
        let context = libc::malloc(size_of::<TaskContext>()).cast::<TaskContext>();
        if context.is_null() {
            chimi_runtime_exhausted_memory();
        }
        context.write(TaskContext {
            runtime: call.runtime,
            environment: ptr::null_mut(),
            entry: ptr::null(),
            captures,
            span: call.span,
            parent: 0,
        });
        let mut request = SchedulerRequest::of(Scheduler::REQUEST_TASK_START);
        request.entry = chimi_runtime_task_entry_v1 as *const c_void;
        request.context = context.cast();
        let cancel = scheduler::request(&mut request);
        if request.status != Scheduler::RESPONSE_OK {
            fragment::free(current_carrier(), captures);
            libc::free(context.cast());
            if cancel {
                return cancelled();
            }
            return defect(c"Task.start scheduler request failed", node);
        }
        if cancel {
            return cancelled();
        }
        // The child has not run yet: it adopts the fragment when it first
        // runs, so the transfer rewrites the capabilities in the child's copy,
        // and the context is still the child's to read.
        (*context).parent = request.owner;
        let mut transfer_cancelled = false;
        if !chimi_runtime_transfer_to_child_v1((*captures).roots[0].cast(), request.task, &raw mut transfer_cancelled) {
            if transfer_cancelled {
                return cancelled();
            }
            return defect(c"Task.start could not transfer an exclusive capture", node);
        }
        success(task_value(request.task))
    }
}

/// The children of a combinator still running: `waiting[..live]`, with each
/// one's index in the input list in `positions`.
struct Children {
    waiting: *mut u64,
    positions: *mut u32,
    live: u32,
}

impl Children {
    fn new(count: u32) -> Self {
        // SAFETY: plain allocations, checked below.
        let (waiting, positions) = unsafe {
            (
                libc::malloc(count as usize * size_of::<u64>()).cast::<u64>(),
                libc::malloc(count as usize * size_of::<u32>()).cast::<u32>(),
            )
        };
        if waiting.is_null() || positions.is_null() {
            // SAFETY: exits.
            unsafe { chimi_runtime_exhausted_memory() }
        }
        Self { waiting, positions, live: 0 }
    }

    /// # Safety
    /// Fewer than the capacity are live.
    unsafe fn push(&mut self, task: u64, position: u32) {
        // SAFETY: the caller's contract.
        unsafe {
            *self.waiting.add(self.live as usize) = task;
            *self.positions.add(self.live as usize) = position;
        }
        self.live += 1;
    }

    /// Cancels every child still live; false when this task was cancelled
    /// meanwhile.
    ///
    /// # Safety
    /// Runs on a fiber of a scheduled program.
    unsafe fn cancel_live(&self) -> bool {
        for index in 0..self.live as usize {
            let mut request = SchedulerRequest::of(Scheduler::REQUEST_TASK_CANCEL);
            // SAFETY: below `live`; the caller's contract.
            unsafe {
                request.task = *self.waiting.add(index);
                if scheduler::request(&mut request) {
                    return false;
                }
            }
        }
        true
    }
}

impl Drop for Children {
    fn drop(&mut self) {
        // SAFETY: both came from malloc.
        unsafe {
            libc::free(self.waiting.cast());
            libc::free(self.positions.cast());
        }
    }
}

/// What waiting for the next child gave.
enum Next {
    /// The child's outcome, at its index in the input list.
    Outcome(*mut Control, u32),
    /// The child ended cancelled.
    Cancelled,
    /// This task must return the control at once.
    Interrupted(*mut Control),
}

/// Waits for the first live child to terminate and removes it.
///
/// # Safety
/// Runs on a fiber of a scheduled program; at least one child is live.
unsafe fn next(children: &mut Children, drain: bool, node: i64) -> Next {
    // SAFETY: the caller's contract; indexes stay below `live`.
    unsafe {
        loop {
            let mut request =
                SchedulerRequest::of(if drain { Scheduler::REQUEST_TASK_DRAIN } else { Scheduler::REQUEST_TASK_ANY });
            request.value = i64::from(children.live);
            request.context = children.waiting.cast();
            if scheduler::request(&mut request) {
                return Next::Interrupted(cancelled());
            }
            if request.status == Scheduler::RESPONSE_PENDING {
                continue;
            }
            let live = children.live as usize;
            let found = (0..live).find(|&index| *children.waiting.add(index) == request.task);
            let Some(found) = found.filter(|_| {
                request.status != Scheduler::RESPONSE_ERROR && request.status != Scheduler::RESPONSE_UNSET
            }) else {
                return Next::Interrupted(defect(c"Task combinator scheduler request failed", node));
            };
            let position = *children.positions.add(found);
            children.live -= 1;
            let last = children.live as usize;
            *children.waiting.add(found) = *children.waiting.add(last);
            *children.positions.add(found) = *children.positions.add(last);
            return match request.status {
                Scheduler::RESPONSE_SUCCEEDED => Next::Outcome(task_reply(request.value), position),
                Scheduler::RESPONSE_CANCELLED => Next::Cancelled,
                Scheduler::RESPONSE_DEFECTED if request.value > 0 => {
                    Next::Outcome(task_reply(request.value), position)
                }
                Scheduler::RESPONSE_DEFECTED => {
                    Next::Interrupted(defect(c"Task combinator child defected", node))
                }
                Scheduler::RESPONSE_STACK_OVERFLOW => Next::Outcome(defect(c"Task stack overflow", node), position),
                _ => Next::Interrupted(defect(c"Task combinator scheduler request failed", node)),
            };
        }
    }
}

/// `Ok(value)`, or `Err(error)` keeping the failure's cause, as
/// `Result.capture` builds them.
///
/// # Safety
/// Runs on a fiber that may allocate; `outcome` is a live success or failure.
unsafe fn stored_outcome(outcome: *mut Control, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let failed = (*outcome).kind == ControlKind::FAILURE;
        let constructor = if failed { StandardConstructor::RESULT_ERR } else { StandardConstructor::RESULT_OK };
        let stored = standard_value(constructor, &[(*outcome).value]);
        if stored.is_null() {
            return defect(c"Result is missing from the program", node);
        }
        success(if failed { with_stored_cause(stored, control_cause(outcome)) } else { stored })
    }
}

/// `Task.all`, `Task.results` or `Task.race` over the callables in
/// `operations`.
///
/// Each operation runs as a child of the calling task. Once the outcome is
/// decided (`Task.all`'s first expected failure, `Task.race`'s first
/// success or failure, or a child cancelled or overflowing), the rest are
/// cancelled and drained with TASK_DRAIN, which is not a cancellation point:
/// a scope that fires meanwhile cannot replace the decided outcome.
/// `Task.all` keeps the failures of siblings that terminated before their
/// cancellation took effect as suppressed causes with reason Sibling, in
/// termination order.
///
/// A child defect needs no code here: the scheduler unwinds this task's
/// containment when the child terminates, which resumes this fiber with a
/// terminal cancel.
///
/// # Safety
/// As for [`task_operation`]; `operations` is a List.
unsafe fn combine(call: TaskCall, intrinsic: u32, operations: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = i64::from(call.call_node);
        let count = (*operations).payload.list.length;
        if count == 0 {
            if intrinsic == Intrinsic::TASK_RACE {
                return defect(c"Task.race needs at least one operation", node);
            }
            return success(list::list_value(list::from_values(ptr::null(), 0), 0, 0));
        }
        let mut children = Children::new(count);
        for index in 0..count {
            let operation = list::get(operations, index);
            if (*operation).kind != ValueKind::CALLABLE {
                return shape_defect(call.call_node);
            }
            // A child that cannot start ends this call; the ones already
            // started are this task's children, which its unwinding cancels
            // and drains.
            let started = start(call, operation);
            if (*started).kind != ControlKind::SUCCESS {
                return started;
            }
            children.push((*(*started).value).payload.task, index);
        }

        let values = new_value_slots(count);
        let mut decided: *mut Control = ptr::null_mut();
        while children.live > 0 && decided.is_null() {
            match next(&mut children, false, node) {
                Next::Interrupted(interrupted) => {
                    // A child defect reaches here only when this task absorbs
                    // it (a process body that is shutting down); the siblings
                    // stop now, not when the body ends. After a terminal cancel
                    // (a forced body, Task.cancel, a parent's end) the scheduler
                    // ends the children itself, so no request is issued: a
                    // forced body that parked now would have its fiber and heap
                    // abandoned (CHI-265).
                    if !((*interrupted).kind == ControlKind::CANCEL && cancel_is_terminal()) {
                        children.cancel_live();
                    }
                    return interrupted;
                }
                // A child that ends cancelled cancels this call, as
                // `Task.result` of it would: a scope or shutdown cancelled this
                // task too, or the child ran `Task.result` on a task it
                // cancelled itself.
                Next::Cancelled => decided = cancelled(),
                Next::Outcome(outcome, position) => {
                    let kind = (*outcome).kind;
                    if kind != ControlKind::SUCCESS && kind != ControlKind::FAILURE {
                        decided = outcome;
                    } else if intrinsic == Intrinsic::TASK_RESULTS {
                        let stored = stored_outcome(outcome, node);
                        if (*stored).kind != ControlKind::SUCCESS {
                            decided = stored;
                        } else {
                            *values.add(position as usize) = (*stored).value;
                        }
                    } else if intrinsic == Intrinsic::TASK_RACE || kind == ControlKind::FAILURE {
                        decided = outcome;
                    } else {
                        *values.add(position as usize) = (*outcome).value;
                    }
                }
            }
        }

        if children.live > 0 {
            if !children.cancel_live() {
                return cancelled();
            }
            let collect_siblings = intrinsic == Intrinsic::TASK_ALL && (*decided).kind == ControlKind::FAILURE;
            let mut cause = if collect_siblings { control_cause(decided) } else { ptr::null_mut() };
            let mut suppressed = false;
            while children.live > 0 {
                match next(&mut children, true, node) {
                    Next::Interrupted(interrupted) => return interrupted,
                    Next::Cancelled => {}
                    Next::Outcome(outcome, _) => {
                        if collect_siblings && (*outcome).kind == ControlKind::FAILURE {
                            cause = cause_with_sibling(cause, control_cause(outcome));
                            suppressed = true;
                        }
                    }
                }
            }
            if suppressed {
                decided = control_with_cause(decided, cause);
            }
        }
        if !decided.is_null() {
            return decided;
        }
        success(list::list_value(list::from_values(values, count), 0, count))
    }
}
