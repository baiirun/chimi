//! The supervisor starts (docs/architecture/supervised-process-runtime.md,
//! "Mapping onto the native runtime"): `Supervisor.start`,
//! `start_supervisor`, `start_process` and `start_handoff`. Each marshals
//! the Chimi spec or policy records into the scheduler's C structs and asks
//! the scheduler to start them. Moved from `runtime.c`, which keeps every
//! incarnation's entry, `chimi_rt_process_entry`, and the release of a
//! spec's context.
//!
//! Only the process kernel reaches this file, so only process programs
//! link it (lab MF06).

use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_void};
use core::mem::size_of;
use core::ptr;

use super::channels::fragment_of;
use super::message::Message;
use super::process_kernel::hand_over;
use super::processes::field;
use super::standard::{defect, defect_with, failure, shape_defect, standard_constructor_of, standard_value, success};
use crate::eval::{FOREIGN_POINTER_DEFECT, cancelled};
use crate::heap::fragment;
use crate::heap::layout::{
    Control, Intrinsic, ProcessRequest, ProcessSpec, ProcessSpecContext, Runtime, Scheduler, StandardConstructor,
    SupervisorMember, SupervisorPolicy, Value, ValueKind,
};
use crate::heap::allocator::current_carrier;
use crate::heap::list;
use crate::heap::values::{new_value, text_value};
use crate::scheduler::process_request;

unsafe extern "C" {
    /// The trace label of a scheduler name id, or null for an id that names
    /// no `process name`.
    fn chimi_runtime_process_name_label_v1(name: u64) -> *const c_char;
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// What a start's request reads beside its arguments: the runtime and the
/// entry every incarnation runs.
#[derive(Clone, Copy)]
pub(crate) struct Starter {
    pub(crate) runtime: *mut Runtime,
    /// `chimi_rt_process_entry`, which only the process kernel's runtime
    /// defines, so runtime.c passes it in.
    pub(crate) entry: *const c_void,
}

/// The C structs one start request reads. The arrays are freed after the
/// request; the contexts belong to the scheduler once the request is
/// issued, served or not, so a start frees only those it never sent.
struct Marshal {
    arrays: Vec<*mut c_void>,
    contexts: Vec<*mut ProcessSpecContext>,
}

fn keep<T>(list: &mut Vec<*mut T>, item: *mut T) {
    if list.try_reserve(1).is_err() {
        // SAFETY: ends the program.
        unsafe { chimi_runtime_exhausted_memory() }
    }
    list.push(item);
}

/// A spec's context, as runtime.c's `release_spec_context` frees it.
///
/// # Safety
/// `context` came from [`Marshal::spec`] and was never sent.
unsafe fn release_context(context: *mut ProcessSpecContext) {
    // SAFETY: the caller's contract; the body is unadopted.
    unsafe {
        fragment::free(current_carrier(), (*context).body);
        libc::free(context.cast());
    }
}

impl Marshal {
    const fn new() -> Self {
        Self { arrays: Vec::new(), contexts: Vec::new() }
    }

    /// `count` zeroed `T`s, at least one, freed by [`Marshal::finish`].
    fn allocate<T>(&mut self, count: usize) -> *mut T {
        // SAFETY: calloc checks the product.
        let items = unsafe { libc::calloc(count.max(1), size_of::<T>()) };
        if items.is_null() {
            // SAFETY: ends the program.
            unsafe { chimi_runtime_exhausted_memory() }
        }
        keep(&mut self.arrays, items);
        items.cast()
    }

    /// Frees the arrays, and the contexts too unless the request was issued.
    ///
    /// # Safety
    /// When `handed_over` is false, no request named the contexts.
    unsafe fn finish(self, handed_over: bool) {
        for array in self.arrays {
            // SAFETY: each came from calloc.
            unsafe { libc::free(array) };
        }
        if !handed_over {
            for context in self.contexts {
                // SAFETY: the caller's contract.
                unsafe { release_context(context) };
            }
        }
    }

    /// The scheduler's form of the `Process.Spec` record `spec`, or the defect
    /// that stops the start `function`.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate; `spec` is live or null.
    unsafe fn spec(
        &mut self,
        starter: Starter,
        spec: *mut Value,
        function: &[u8],
        call_node: u32,
    ) -> Result<ProcessSpec, *mut Control> {
        // SAFETY: the caller's contract; payloads are read as their kinds say.
        unsafe {
            let node = i64::from(call_node);
            let body = field(spec, c"body");
            let capacity = field(spec, c"capacity");
            let name = field(spec, c"name");
            let within = field(spec, c"shutdown_within");
            if !is(body, ValueKind::CALLABLE)
                || !is(capacity, ValueKind::INTEGER)
                || !is(name, ValueKind::CONSTRUCTED)
                || !is(within, ValueKind::DURATION)
            {
                return Err(shape_defect(call_node));
            }
            let capacity = (*capacity).payload.integer;
            if capacity > i64::from(u32::MAX) {
                return Err(too_large(function, b": mailbox capacity must be at most 4294967295, got ", capacity, node));
            }
            let Some(copy) = fragment_of(body) else { return Err(defect(FOREIGN_POINTER_DEFECT, node)) };
            let context = libc::malloc(size_of::<ProcessSpecContext>()).cast::<ProcessSpecContext>();
            if context.is_null() {
                chimi_runtime_exhausted_memory();
            }
            let handoff = field(spec, c"handoff");
            context.write(ProcessSpecContext {
                runtime: starter.runtime,
                body: copy,
                handoff: is(handoff, ValueKind::BOOLEAN) && (*handoff).payload.boolean,
            });
            keep(&mut self.contexts, context);
            let name = (*name).payload.constructed;
            let mut registered = 0;
            if name.length == 1 {
                let address = (*name.fields).value;
                if (*address).kind == ValueKind::PROCESS_ADDRESS {
                    registered = (*address).payload.address.target;
                }
            }
            Ok(ProcessSpec {
                entry: starter.entry,
                context: context.cast(),
                mailbox_capacity: capacity as u32,
                shutdown_within: (*within).payload.integer,
                name: registered,
            })
        }
    }

    /// The scheduler's form of the `Supervisor.Policy` record `policy`, with
    /// its members' specs and nested policies, or the defect that stops the
    /// start `function`.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate; `policy` is live or null.
    unsafe fn policy(
        &mut self,
        starter: Starter,
        policy: *mut Value,
        function: &[u8],
        call_node: u32,
    ) -> Result<SupervisorPolicy, *mut Control> {
        // SAFETY: the caller's contract; payloads are read as their kinds say.
        unsafe {
            let node = i64::from(call_node);
            let members = field(policy, c"members");
            let capacity = field(policy, c"capacity");
            let restarts = field(policy, c"restarts");
            let limit = field(restarts, c"limit");
            let window = field(restarts, c"window");
            if !is(members, ValueKind::LIST)
                || !is(capacity, ValueKind::INTEGER)
                || !is(limit, ValueKind::INTEGER)
                || !is(window, ValueKind::DURATION)
            {
                return Err(shape_defect(call_node));
            }
            let capacity = (*capacity).payload.integer;
            if capacity > i64::from(u32::MAX) {
                return Err(too_large(function, b": capacity must be at most 4294967295, got ", capacity, node));
            }
            let limit = (*limit).payload.integer;
            if limit > i64::from(u32::MAX) {
                return Err(too_large(function, b": restart limit must be at most 4294967295, got ", limit, node));
            }
            let count = (*members).payload.list.length;
            let array = self.allocate::<SupervisorMember>(count as usize);
            for index in 0..count {
                let member = list::get(members, index);
                let spec = field(member, c"spec");
                let restart = restart_of(field(member, c"restart"));
                let slot = array.add(index as usize);
                if spec.is_null() {
                    let child = self.allocate::<SupervisorPolicy>(1);
                    child.write(self.policy(starter, field(member, c"policy"), function, call_node)?);
                    (*slot).kind = Scheduler::MEMBER_SUPERVISOR;
                    (*slot).restart = restart;
                    (*slot).supervisor = child;
                } else {
                    (*slot).process = self.spec(starter, spec, function, call_node)?;
                    (*slot).kind = Scheduler::MEMBER_PROCESS;
                    (*slot).restart = restart;
                }
            }
            Ok(SupervisorPolicy {
                members: array,
                member_count: count as usize,
                capacity: capacity as u32,
                restart_limit: limit as u32,
                restart_within: (*window).payload.integer,
            })
        }
    }
}

/// # Safety
/// `value` is live or null.
unsafe fn is(value: *mut Value, kind: ValueKind) -> bool {
    // SAFETY: the caller's contract.
    !value.is_null() && unsafe { (*value).kind } == kind
}

/// `<function><heading><value>`, as a defect.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn too_large(function: &[u8], heading: &[u8], value: i64, node: i64) -> *mut Control {
    let message: *const c_char = Message::new().text(function).text(heading).integer(value).finish();
    // SAFETY: the caller's contract.
    unsafe { defect_with(message, node) }
}

/// The scheduler's restart code for a `Supervisor.Restart` value.
///
/// # Safety
/// `restart` is live or null.
unsafe fn restart_of(restart: *mut Value) -> u32 {
    // SAFETY: the caller's contract; a constructed value's constructor is a
    // symbol of the program.
    let on_defect = unsafe {
        is(restart, ValueKind::CONSTRUCTED)
            && standard_constructor_of((*restart).payload.constructed.constructor)
                == StandardConstructor::RESTART_ON_DEFECT
    };
    if on_defect { Scheduler::RESTART_ON_DEFECT } else { Scheduler::RESTART_NEVER }
}

/// A Supervisor or managed member handle.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn managed_value(kind: ValueKind, supervisor: u64, membership: u64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(kind);
        (*value).payload.managed.supervisor = supervisor;
        (*value).payload.managed.membership = membership;
        value
    }
}

/// What a refused `Supervisor.start`, `start_supervisor` or
/// `start_process` returns.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn start_error(status: u32, name: u64, function: &[u8], node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        match status {
            Scheduler::RESPONSE_CAPACITY_REACHED => {
                failure(standard_value(StandardConstructor::START_CAPACITY_REACHED, &[]))
            }
            Scheduler::RESPONSE_STOPPED => failure(standard_value(StandardConstructor::START_STOPPED, &[])),
            Scheduler::RESPONSE_NAME_TAKEN => {
                let label = chimi_runtime_process_name_label_v1(name);
                let label = if label.is_null() { c"" } else { CStr::from_ptr(label) };
                let text = text_value(label.as_ptr(), label.count_bytes());
                failure(standard_value(StandardConstructor::START_NAME_TAKEN, &[text]))
            }
            _ => {
                let message: *const c_char =
                    Message::new().text(function).text(b" scheduler request failed").finish();
                defect_with(message, node)
            }
        }
    }
}

/// `Supervisor.start` and `Supervisor.start_supervisor`: a root supervisor,
/// or a supervisor nested under the running one `arguments[0]`.
///
/// # Safety
/// Runs on a fiber of a program linked with the process kernel; the
/// arguments are live.
unsafe fn start_supervisor(starter: Starter, nested: bool, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let function: &[u8] = if nested { b"Supervisor.start_supervisor" } else { b"Supervisor.start" };
        if arguments.len() != if nested { 3 } else { 1 }
            || (nested && (*arguments[0]).kind != ValueKind::SUPERVISOR_RUNNING)
        {
            return shape_defect(call_node);
        }
        let mut marshal = Marshal::new();
        let policy = match marshal.policy(starter, arguments[usize::from(nested)], function, call_node) {
            Ok(policy) => policy,
            Err(failed) => {
                marshal.finish(false);
                return failed;
            }
        };
        let parent = if nested { (*arguments[0]).payload.managed.supervisor } else { 0 };
        let kind =
            if nested { Scheduler::REQUEST_SUPERVISOR_START_SUPERVISOR } else { Scheduler::REQUEST_SUPERVISOR_START };
        let mut request = ProcessRequest::of(kind);
        request.supervisor = parent;
        request.restart = if nested { restart_of(arguments[2]) } else { 0 };
        request.policy = (&raw const policy).cast();
        let cancel = process_request(&mut request);
        marshal.finish(true);
        if cancel {
            return cancelled();
        }
        if request.status != Scheduler::RESPONSE_OK {
            return start_error(request.status, request.name, function, node);
        }
        success(if nested {
            managed_value(ValueKind::MANAGED_SUPERVISOR, parent, request.membership)
        } else {
            managed_value(ValueKind::SUPERVISOR_RUNNING, request.supervisor, 0)
        })
    }
}

/// `Supervisor.start_process`: one process under the running supervisor
/// `arguments[0]`.
///
/// # Safety
/// As for [`start_supervisor`].
unsafe fn start_process(starter: Starter, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        let function: &[u8] = b"Supervisor.start_process";
        if arguments.len() != 3 || (*arguments[0]).kind != ValueKind::SUPERVISOR_RUNNING {
            return shape_defect(call_node);
        }
        let mut marshal = Marshal::new();
        let spec = match marshal.spec(starter, arguments[1], function, call_node) {
            Ok(spec) => spec,
            Err(failed) => {
                marshal.finish(false);
                return failed;
            }
        };
        let supervisor = (*arguments[0]).payload.managed.supervisor;
        let mut request = ProcessRequest::of(Scheduler::REQUEST_SUPERVISOR_START_PROCESS);
        request.supervisor = supervisor;
        request.restart = restart_of(arguments[2]);
        request.spec = (&raw const spec).cast();
        let cancel = process_request(&mut request);
        marshal.finish(true);
        if cancel {
            return cancelled();
        }
        if request.status != Scheduler::RESPONSE_OK {
            return start_error(request.status, request.name, function, node);
        }
        success(managed_value(ValueKind::MANAGED_PROCESS, supervisor, request.membership))
    }
}

/// `Supervisor.start_handoff`: one process under the running supervisor
/// `arguments[0]` whose first incarnation takes `arguments[2]`. The
/// argument travels like a `Process.send` message: a copy whose Receivers
/// and Completions go in flight once the start is accepted.
///
/// # Safety
/// As for [`start_supervisor`].
unsafe fn start_handoff(starter: Starter, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        let node = i64::from(call_node);
        if arguments.len() != 3 || (*arguments[0]).kind != ValueKind::SUPERVISOR_RUNNING {
            return shape_defect(call_node);
        }
        let mut marshal = Marshal::new();
        let spec = match marshal.spec(starter, arguments[1], b"Supervisor.start_handoff", call_node) {
            Ok(spec) => spec,
            Err(failed) => {
                marshal.finish(false);
                return failed;
            }
        };
        let supervisor = (*arguments[0]).payload.managed.supervisor;
        let mut request = ProcessRequest::of(Scheduler::REQUEST_SUPERVISOR_START_PROCESS);
        request.supervisor = supervisor;
        request.restart = Scheduler::RESTART_NEVER;
        request.spec = (&raw const spec).cast();
        let Some(cancel) = hand_over(&mut request, arguments[2], false) else {
            marshal.finish(false);
            return defect(FOREIGN_POINTER_DEFECT, node);
        };
        marshal.finish(true);
        if cancel {
            return cancelled();
        }
        match request.status {
            Scheduler::RESPONSE_OK => success(managed_value(ValueKind::MANAGED_PROCESS, supervisor, request.membership)),
            Scheduler::RESPONSE_CAPACITY_REACHED => {
                failure(standard_value(StandardConstructor::HANDOFF_CAPACITY_REACHED, &[arguments[2]]))
            }
            Scheduler::RESPONSE_STOPPED => {
                failure(standard_value(StandardConstructor::HANDOFF_STOPPED, &[arguments[2]]))
            }
            _ => defect(c"Supervisor.start_handoff scheduler request failed", node),
        }
    }
}

/// The supervisor start `intrinsic` on `arguments`, or null for another
/// intrinsic.
///
/// # Safety
/// Runs on a fiber of a program linked with the process kernel; the
/// arguments are live.
pub(crate) unsafe fn start_operation(
    starter: Starter,
    intrinsic: u32,
    arguments: &[*mut Value],
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        match intrinsic {
            Intrinsic::SUPERVISOR_START => start_supervisor(starter, false, arguments, call_node),
            Intrinsic::SUPERVISOR_START_SUPERVISOR => start_supervisor(starter, true, arguments, call_node),
            Intrinsic::SUPERVISOR_START_PROCESS => start_process(starter, arguments, call_node),
            Intrinsic::SUPERVISOR_START_HANDOFF => start_handoff(starter, arguments, call_node),
            _ => ptr::null_mut(),
        }
    }
}
