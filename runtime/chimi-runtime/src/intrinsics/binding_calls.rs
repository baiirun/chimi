//! The call half of the C binding API (`chimi_binding.h`): calling a
//! binding function, the handles its calls hold, closes, cancellation
//! checks, and waits on a descriptor or a child (CHI-305 step 5, phase 8).
//! The value half is [`super::binding_values`]; the slots are
//! [`crate::handles`], reached through [`crate::handles_abi`].
//!
//! Handles: a slot per live resource. A value names its slot and the
//! slot's generation, so a copy of a closed handle stays closed when the
//! slot is reused. Slot 0 is never used, so a zeroed value is never open.
//!
//! A handle value may reach tasks on any carrier, so the table is behind
//! [`chimi_runtime_binding_handle_lock`], which is taken after the kernel
//! lock (a count's zero, [`chimi_runtime_native_handle_unreferenced_v1`],
//! arrives in scheduler hooks) and is never held across a scheduler request
//! or a type's release. A binding call holds every open handle among its
//! arguments for its whole length ([`hold`]), so a close by another task,
//! or by the call itself, only marks the handle closed: its state stays
//! valid until the last call holding it returns, which releases it.
//!
//! One binding call at a time runs on a handle (its runner), so a binding's
//! state needs no lock of its own when tasks on several carriers share the
//! handle. A call takes every handle it holds when it starts, waiting on
//! [`HANDLE_FREE`] while another call runs on any of them, and gives them
//! back while it waits on a descriptor or a child or makes any other
//! scheduler request ([`run`], [`request`]), so calls on one handle
//! interleave only where a task can park, as they do on one carrier. A call
//! that would wait for a handle blocks its carrier's thread, not just its
//! task: binding code between parks is meant to be short.

use core::cell::UnsafeCell;
use core::ffi::{c_int, c_void};
use core::ptr;

use super::standard::{defect, symbol};
use crate::handles_abi::{
    CLOSE_NOT_OPEN, ChimiHandleRelease, chimi_handles_close_v1, chimi_handles_count_v1, chimi_handles_end_v1,
    chimi_handles_hold_v1, chimi_handles_is_open_v1, chimi_handles_let_go_v1, chimi_handles_open_v1,
    chimi_handles_runner_v1, chimi_handles_set_runner_v1, chimi_handles_state_v1,
};
use crate::heap::layout::{Binding, BindingCall, BindingType, Control, ControlKind, Runtime, Scheduler, SchedulerRequest, Value, ValueKind};
use crate::heap::values::{new_control, new_value};
use crate::scheduler;

unsafe extern "C" {
    /// The program's bindings (program data), `CHIMI_BINDING_COUNT` of them.
    static CHIMI_BINDINGS: [Binding; 0];
    static CHIMI_BINDING_COUNT: u32;
    fn chimi_runtime_retain_counted_value(value: *const Value);
    fn chimi_runtime_exhausted_memory() -> !;
    fn abort() -> !;
}

/// `chimi_binding.h`'s wait answers and interests.
const READY: c_int = 0;
const WAIT_CANCELLED: c_int = 1;
const WAIT_FAILED: c_int = 2;
const WAIT_CLOSED: c_int = 3;
const READABLE: c_int = 1;
const WRITABLE: c_int = 2;
const NO_HANDLE: u32 = u32::MAX;
/// The high bit of a counted native handle's key (runtime.c's
/// `NATIVE_HANDLE_KEY`).
const NATIVE_HANDLE_KEY: u64 = 1 << 62;

/// A pthread mutex in a static, for the C stress harness to take too.
#[repr(transparent)]
pub struct HandleLock(UnsafeCell<libc::pthread_mutex_t>);

// SAFETY: pthread mutexes are made to be shared between threads.
unsafe impl Sync for HandleLock {}

struct HandleFree(UnsafeCell<libc::pthread_cond_t>);

// SAFETY: as for `HandleLock`.
unsafe impl Sync for HandleFree {}

/// The handle table's lock. The C stress harness (carrier-threads-stress.c)
/// takes it to read the table directly.
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static chimi_runtime_binding_handle_lock: HandleLock =
    HandleLock(UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER));
/// Signalled when a call stops running on a handle.
static HANDLE_FREE: HandleFree = HandleFree(UnsafeCell::new(libc::PTHREAD_COND_INITIALIZER));

fn lock() {
    // SAFETY: the mutex is static; a failure would leave the table unguarded.
    unsafe {
        if libc::pthread_mutex_lock(chimi_runtime_binding_handle_lock.0.get()) != 0 {
            abort();
        }
    }
}

fn unlock() {
    // SAFETY: the caller holds the mutex.
    unsafe {
        if libc::pthread_mutex_unlock(chimi_runtime_binding_handle_lock.0.get()) != 0 {
            abort();
        }
    }
}

const NO_RELEASE: ChimiHandleRelease = ChimiHandleRelease { kind: ptr::null(), state: ptr::null_mut() };

/// Runs a state's release, once the lock is let go.
fn release_run(release: ChimiHandleRelease) {
    let kind = release.kind.cast::<BindingType>();
    if kind.is_null() {
        return;
    }
    // SAFETY: a kind the table hands back is a live binding type.
    unsafe {
        if let Some(release_state) = (*kind).release {
            release_state(release.state);
        }
    }
}

/// The scheduler's key for the waits on a handle's descriptors: nonzero,
/// since slot 0 is unused, and distinct per generation. The scheduler reads
/// it as slot and generation, and treats a withdraw as final for that
/// generation and older ones of the slot (CHIMI_REQUEST_IO_WITHDRAW), so a
/// slot's generation must only grow.
fn resource_key(slot: u32, generation: u32) -> u64 {
    u64::from(slot) << 32 | u64::from(generation)
}

/// Argument `index` of `call` when it is a native handle, else null.
///
/// # Safety
/// `call` is the running binding call.
unsafe fn handle_argument(call: *const BindingCall, index: u32) -> *const Value {
    // SAFETY: the caller's contract: the call holds `argument_count` live values.
    unsafe {
        if index >= (*call).argument_count {
            return ptr::null();
        }
        let value = *(*call).arguments.add(index as usize);
        if (*value).kind == ValueKind::NATIVE_HANDLE { value } else { ptr::null() }
    }
}

/// Argument `index`'s slot; the caller knows it is a handle.
///
/// # Safety
/// `call` is the running binding call and `index` one of its arguments.
unsafe fn slot_of(call: *const BindingCall, index: u32) -> u32 {
    // SAFETY: the caller's contract.
    unsafe { (**(*call).arguments.add(index as usize)).payload.handle.slot }
}

/// Whether `call` holds argument `index`.
///
/// # Safety
/// `call` is the running binding call and `index` one of its arguments.
unsafe fn held(call: *const BindingCall, index: u32) -> bool {
    // SAFETY: the caller's contract: `held` has `argument_count` entries.
    unsafe { *(*call).held.add(index as usize) }
}

/// Closes the handle (slot, generation) of `kind` (null for any) when it is
/// open. A close from a binding call (`call`; null from a collection or a
/// hook) then wakes every task waiting on the handle's descriptors, so none
/// stays parked on a descriptor the release frees for reuse; each answers
/// CHIMI_BINDING_WAIT_CLOSED. The handle reads as closed before the
/// withdraw, so a waiter that wakes sees it closed. The state is released
/// here unless a binding call holds the handle.
///
/// # Safety
/// `call` is null or the running binding call.
unsafe fn close(slot: u32, generation: u32, kind: *const BindingType, call: *const BindingCall) {
    let mut release = NO_RELEASE;
    lock();
    // SAFETY: the lock is held; `release` is writable.
    let closed = unsafe { chimi_handles_close_v1(slot, generation, kind.cast(), &mut release) } != CLOSE_NOT_OPEN;
    unlock();
    // The answer is always OK: the withdraw never parks. A forced process
    // body that gets CANCEL here is abandoned at its next park, so
    // releasing anyway is safe.
    if closed && !call.is_null() && scheduler::requires_scheduler() {
        let mut withdraw = SchedulerRequest::of(Scheduler::REQUEST_IO_WITHDRAW);
        withdraw.channel = resource_key(slot, generation);
        // SAFETY: the caller's contract.
        unsafe { request(call, &mut withdraw) };
    }
    release_run(release);
}

/// Closes the handle `key` names when it is still open: no value names it
/// any more. No task can be waiting on its descriptors, since a waiting
/// binding call holds the handle as an argument, so nothing is withdrawn;
/// this runs inside collections and scheduler hooks, where no request may
/// be issued.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_native_handle_unreferenced_v1(key: u64) {
    let slot = ((key & !NATIVE_HANDLE_KEY) >> 32) as u32;
    // SAFETY: a null call issues no request.
    unsafe { close(slot, key as u32, ptr::null(), ptr::null()) };
}

/// Runs every handle's release once, when the program ends and nothing
/// runs: the open ones, and the ones closed while held. A call can still
/// count as holding a handle then, when its task was abandoned inside it (a
/// forced process body that parked again), so holders are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_release_binding_handles_v1() {
    let mut slot: u32 = 1;
    loop {
        let mut release = NO_RELEASE;
        lock();
        // SAFETY: the lock is held.
        if slot >= unsafe { chimi_handles_count_v1() } {
            unlock();
            return;
        }
        // SAFETY: the lock is held; `release` is writable.
        unsafe { chimi_handles_end_v1(slot, &mut release) };
        unlock();
        release_run(release);
        slot += 1;
    }
}

/// True when some call runs on a handle `call` holds. `call` itself never
/// does here: it takes its handles only when it runs on none of them.
///
/// # Safety
/// The caller holds the lock; `call` is the running binding call.
unsafe fn run_blocked(call: *const BindingCall) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        (0..(*call).argument_count)
            .any(|index| held(call, index) && !chimi_handles_runner_v1(slot_of(call, index)).is_null())
    }
}

/// Starts (`running`) or stops `call` running on every handle it holds.
/// Starting waits until no other call runs on any of them and then takes
/// them all at once, so two calls sharing handles never each wait for the
/// other.
///
/// # Safety
/// The caller holds the lock; `call` is the running binding call.
unsafe fn run_locked(call: *const BindingCall, running: bool) {
    // SAFETY: the caller's contract; the condition and mutex are static.
    unsafe {
        if running {
            while run_blocked(call) {
                libc::pthread_cond_wait(HANDLE_FREE.0.get(), chimi_runtime_binding_handle_lock.0.get());
            }
        }
        let runner = if running { call.cast::<c_void>() } else { ptr::null() };
        let mut any = false;
        for index in 0..(*call).argument_count {
            if held(call, index) {
                chimi_handles_set_runner_v1(slot_of(call, index), runner);
                any = true;
            }
        }
        if any && !running {
            libc::pthread_cond_broadcast(HANDLE_FREE.0.get());
        }
    }
}

/// Lets the call's handles go while it waits (`running` false), or takes
/// them back when the wait ends (`running` true).
///
/// # Safety
/// `call` is the running binding call.
unsafe fn run(call: *const BindingCall, running: bool) {
    lock();
    // SAFETY: the lock is held; the caller's contract.
    unsafe { run_locked(call, running) };
    unlock();
}

/// A scheduler request from inside `call`, made with its handles let go;
/// true when the fiber resumed to be cancelled. Even a request that
/// answers at once parks the fiber, and the scheduler abandons a forced
/// process body at any park, which would leave a handle taken for good;
/// and on one carrier another task may run first, whose call on the handle
/// would then wait on the thread this one needs.
///
/// # Safety
/// `call` is the running binding call, on a fiber.
unsafe fn request(call: *const BindingCall, request: &mut SchedulerRequest) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        run(call, false);
        let cancelled = scheduler::request(request);
        run(call, true);
        cancelled
    }
}

/// Holds (`holding`) or lets go of every open handle among a binding
/// call's arguments, marking the ones held in `call.held`, and starts or
/// stops the call running on them. The last call to let go of a handle
/// closed meanwhile releases its state.
///
/// # Safety
/// `call` is the running binding call.
unsafe fn hold(call: *const BindingCall, holding: bool) {
    // SAFETY: the caller's contract; the lock guards the table.
    unsafe {
        let count = (*call).argument_count;
        lock();
        if holding {
            for index in 0..count {
                let value = *(*call).arguments.add(index as usize);
                let handle = &(*value).payload.handle;
                *(*call).held.add(index as usize) = (*value).kind == ValueKind::NATIVE_HANDLE
                    && chimi_handles_hold_v1(handle.slot, handle.generation);
            }
            run_locked(call, true);
            unlock();
            return;
        }
        // Every let-go is taken back before the binding code goes on.
        for index in 0..count {
            if held(call, index) && chimi_handles_runner_v1(slot_of(call, index)) != call.cast::<c_void>() {
                abort();
            }
        }
        run_locked(call, false);
        let mut releases = [NO_RELEASE; 8];
        let mut release_count = 0;
        for index in 0..count {
            if !held(call, index) {
                continue;
            }
            let mut release = NO_RELEASE;
            if !chimi_handles_let_go_v1(slot_of(call, index), &mut release) {
                continue;
            }
            if release_count == releases.len() {
                // Rare: more closed handles than the buffer holds; run the batch first.
                unlock();
                for pending in releases.iter_mut() {
                    release_run(core::mem::replace(pending, NO_RELEASE));
                }
                release_count = 0;
                lock();
            }
            if let Some(slot) = releases.get_mut(release_count) {
                *slot = release;
            }
            release_count += 1;
        }
        unlock();
        for release in releases.into_iter().take(release_count) {
            release_run(release);
        }
    }
}

/// chimi_binding.h: the state of argument `index` when it is an open
/// handle of `kind`, else null.
///
/// # Safety
/// `call` is the running binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_handle(call: *const BindingCall, index: u32, kind: *const BindingType) -> *mut c_void {
    // SAFETY: the caller's contract.
    let value = unsafe { handle_argument(call, index) };
    if value.is_null() || kind.is_null() {
        return ptr::null_mut();
    }
    lock();
    // SAFETY: the lock is held; `value` is a live handle.
    let state = unsafe { chimi_handles_state_v1((*value).payload.handle.slot, (*value).payload.handle.generation, kind.cast()) };
    unlock();
    state
}

/// chimi_binding.h: a new handle that owns `state`; null when the table is
/// full, after releasing `state`.
///
/// # Safety
/// Runs on a fiber that may allocate; `kind` is a live binding type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_make_handle(
    _call: *mut BindingCall,
    kind: *const BindingType,
    state: *mut c_void,
) -> *mut Value {
    let (mut slot, mut generation) = (0, 0);
    lock();
    // SAFETY: the lock is held; the outputs are writable.
    let opened = unsafe { chimi_handles_open_v1(kind.cast(), state, &mut slot, &mut generation) };
    unlock();
    // SAFETY: the caller's contract.
    unsafe {
        if !opened {
            if let Some(release) = (*kind).release {
                release(state);
            }
            return ptr::null_mut();
        }
        let value = new_value(ValueKind::NATIVE_HANDLE);
        (*value).payload.handle.slot = slot;
        (*value).payload.handle.generation = generation;
        (*value).payload.handle.name = (*kind).name;
        chimi_runtime_retain_counted_value(value);
        value
    }
}

/// chimi_binding.h: closes argument `index` if it is an open handle of
/// `kind`, so later calls see it closed.
///
/// # Safety
/// `call` is the running binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_close_handle(call: *mut BindingCall, index: u32, kind: *const BindingType) {
    // SAFETY: the caller's contract.
    unsafe {
        let value = handle_argument(call, index);
        if !value.is_null() {
            close((*value).payload.handle.slot, (*value).payload.handle.generation, kind, call);
        }
    }
}

/// chimi_binding.h: whether the call's task was cancelled; under the
/// scheduler this is a cancellation point.
///
/// # Safety
/// `call` is the running binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_is_cancelled(call: *const BindingCall) -> c_int {
    // SAFETY: the caller's contract; another carrier may set the flag.
    unsafe {
        if ptr::read_volatile(&raw const (*(*call).runtime.cast::<Runtime>()).cancel) != 0 {
            return 1;
        }
        if !scheduler::requires_scheduler() {
            return 0;
        }
        let mut check = SchedulerRequest::of(Scheduler::REQUEST_SCOPE_CHECK);
        c_int::from(request(call, &mut check))
    }
}

/// True when `handle` names a handle argument that is closed now.
///
/// # Safety
/// `call` is the running binding call.
unsafe fn wait_handle_closed(call: *const BindingCall, handle: u32) -> bool {
    if handle == NO_HANDLE {
        return false;
    }
    // SAFETY: the caller's contract.
    let value = unsafe { handle_argument(call, handle) };
    if value.is_null() {
        return true;
    }
    lock();
    // SAFETY: the lock is held; `value` is a live handle.
    let closed = unsafe { !chimi_handles_is_open_v1((*value).payload.handle.slot, (*value).payload.handle.generation) };
    unlock();
    closed
}

/// The scheduler's key for `handle`'s waits, or 0 for NO_HANDLE.
///
/// # Safety
/// `call` is the running binding call and `handle` NO_HANDLE or one of its
/// handle arguments.
unsafe fn wait_resource(call: *const BindingCall, handle: u32) -> u64 {
    if handle == NO_HANDLE {
        return 0;
    }
    // SAFETY: the caller's contract: the wait checked the handle first.
    unsafe {
        let value = handle_argument(call, handle);
        resource_key((*value).payload.handle.slot, (*value).payload.handle.generation)
    }
}

/// chimi_binding.h: waits until `fd` is ready for `interest`, letting the
/// call's handles go meanwhile.
///
/// # Safety
/// `call` is the running binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_wait(call: *mut BindingCall, handle: u32, fd: c_int, interest: c_int) -> c_int {
    if fd < 0 || !(READABLE..=READABLE | WRITABLE).contains(&interest) {
        return WAIT_FAILED;
    }
    // SAFETY: the caller's contract.
    unsafe {
        if wait_handle_closed(call, handle) {
            return WAIT_CLOSED;
        }
        run(call, false);
        let answer = wait_fd(call, handle, fd, interest);
        run(call, true);
        answer
    }
}

/// chimi_binding_wait's wait, run while the call's handles are let go.
///
/// # Safety
/// `call` is the running binding call.
unsafe fn wait_fd(call: *const BindingCall, handle: u32, fd: c_int, interest: c_int) -> c_int {
    // SAFETY: the caller's contract.
    unsafe {
        if scheduler::requires_scheduler() {
            let resource = wait_resource(call, handle);
            let token = (if interest & READABLE != 0 { Scheduler::IO_READABLE } else { 0 })
                | (if interest & WRITABLE != 0 { Scheduler::IO_WRITABLE } else { 0 });
            loop {
                let mut wait = SchedulerRequest::of(Scheduler::REQUEST_IO_WAIT);
                wait.value = i64::from(fd);
                wait.token = token;
                wait.channel = resource;
                if scheduler::request(&mut wait) {
                    return WAIT_CANCELLED;
                }
                // Another task may have closed the handle while this one was
                // parked; its descriptor number may already belong to a new
                // resource.
                if wait_handle_closed(call, handle) {
                    return WAIT_CLOSED;
                }
                if wait.status == Scheduler::RESPONSE_ERROR {
                    return WAIT_FAILED;
                }
                if wait.status != Scheduler::RESPONSE_PENDING {
                    return READY;
                }
            }
        }
        let events = (if interest & READABLE != 0 { libc::POLLIN } else { 0 })
            | (if interest & WRITABLE != 0 { libc::POLLOUT } else { 0 });
        let mut poll_fd = libc::pollfd { fd, events, revents: 0 };
        loop {
            if ptr::read_volatile(&raw const (*(*call).runtime.cast::<Runtime>()).cancel) != 0 {
                return WAIT_CANCELLED;
            }
            let ready = libc::poll(&mut poll_fd, 1, -1);
            if ready > 0 {
                return if poll_fd.revents & libc::POLLNVAL != 0 { WAIT_FAILED } else { READY };
            }
            if ready < 0 && *errno_location() != libc::EINTR {
                return WAIT_FAILED;
            }
        }
    }
}

/// chimi_binding.h: waits until child `pid` has exited, letting the call's
/// handles go meanwhile; it does not reap the child.
///
/// # Safety
/// `call` is the running binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_wait_child(call: *mut BindingCall, handle: u32, pid: c_int) -> c_int {
    if pid <= 0 {
        return WAIT_FAILED;
    }
    // SAFETY: the caller's contract.
    unsafe {
        if wait_handle_closed(call, handle) {
            return WAIT_CLOSED;
        }
        super::signals::chimi_binding_prepare_children();
        run(call, false);
        let answer = wait_pid(call, handle, pid);
        run(call, true);
        answer
    }
}

/// chimi_binding_wait_child's wait, run while the call's handles are let go.
///
/// # Safety
/// `call` is the running binding call.
unsafe fn wait_pid(call: *const BindingCall, handle: u32, pid: c_int) -> c_int {
    // SAFETY: the caller's contract.
    unsafe {
        if scheduler::requires_scheduler() {
            // The handle's key, so a close of the handle by another task,
            // which withdraws it, wakes this wait: the close only releases
            // the child once this call returns.
            let mut wait = SchedulerRequest::of(Scheduler::REQUEST_CHILD_WAIT);
            wait.value = i64::from(pid);
            wait.channel = wait_resource(call, handle);
            if scheduler::request(&mut wait) {
                return WAIT_CANCELLED;
            }
            if wait_handle_closed(call, handle) {
                return WAIT_CLOSED;
            }
            if wait.status == Scheduler::RESPONSE_ERROR {
                return WAIT_FAILED;
            }
            // Exited, or woken to look again: either way the caller looks.
            // A wake after another task reaped the child is answered from
            // the caller's record, never by waiting on a pid the kernel may
            // have reused.
            return READY;
        }
        loop {
            if ptr::read_volatile(&raw const (*(*call).runtime.cast::<Runtime>()).cancel) != 0 {
                return WAIT_CANCELLED;
            }
            let mut info: libc::siginfo_t = core::mem::zeroed();
            if libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) == 0 {
                return READY;
            }
            if *errno_location() != libc::EINTR {
                return WAIT_FAILED;
            }
        }
    }
}

/// The calling thread's `errno`.
fn errno_location() -> *mut c_int {
    // SAFETY: the C library defines it for every thread.
    unsafe {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let location = libc::__errno_location();
        #[cfg(target_vendor = "apple")]
        let location = libc::__error();
        location
    }
}

/// Calls the reached binding `binding` with `arguments`; its result is the
/// call's outcome. The C stress harness calls it too.
///
/// # Safety
/// Runs on a fiber that may allocate; `runtime` is live and `arguments`
/// holds `count` live values.
#[unsafe(no_mangle)]
#[inline(never)]
pub unsafe extern "C" fn chimi_runtime_invoke_binding_v1(
    runtime: *mut Runtime,
    binding: u32,
    arguments: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract; the table holds CHIMI_BINDING_COUNT entries.
    unsafe {
        if binding >= CHIMI_BINDING_COUNT {
            return defect(c"verified binding call has no binding", i64::from(call_node));
        }
        if ptr::read_volatile(&raw const (*runtime).cancel) != 0 {
            return new_control(ControlKind::CANCEL, ptr::null_mut(), ptr::null(), -1);
        }
        let mut held_small = [false; 8];
        let held = if count as usize <= held_small.len() {
            held_small.as_mut_ptr()
        } else {
            let held = libc::calloc(count as usize, 1).cast::<bool>();
            if held.is_null() {
                chimi_runtime_exhausted_memory();
            }
            held
        };
        let mut call = BindingCall { runtime: runtime.cast(), arguments, argument_count: count, call_node, held };
        hold(&call, true);
        let entry = &*(&raw const CHIMI_BINDINGS).cast::<Binding>().add(binding as usize);
        let control = match entry.function {
            Some(function) => function(&mut call),
            None => ptr::null_mut(),
        };
        hold(&call, false);
        if held != held_small.as_mut_ptr() {
            libc::free(held.cast());
        }
        if control.is_null() {
            return defect(c"a C binding returned no result", i64::from(call_node));
        }
        control
    }
}

/// A call of a binding `native fn`: the callee symbol's `binding` names
/// which.
///
/// # Safety
/// As [`chimi_runtime_invoke_binding_v1`]; `callee` is a symbol index.
pub unsafe fn binding_call(
    runtime: *mut Runtime,
    callee: u32,
    arguments: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { chimi_runtime_invoke_binding_v1(runtime, symbol(callee).binding, arguments, count, call_node) }
}
