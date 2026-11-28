//! The runtime's signal handlers and std/signal's host core.
//!
//! SIGINT and SIGTERM (G22): the first one cancels main. In a program on
//! the scheduler, the scheduler delivers it at its next step: main's
//! children are cancelled and every root supervisor it started stops, each
//! member draining within its shutdown_within. A program without the
//! scheduler sets `cancel`, which its host operations poll; blocking reads
//! return EINTR to see it. A second signal ends the program at once.
//!
//! std/signal (CHI-249): SIGHUP, SIGUSR1 and SIGUSR2 as events a task
//! waits for. Their handler never stops the program: it sets bit `code - 1`
//! of [`USER_SIGNALS`] and wakes the scheduler through the stop signals'
//! pipe; the scheduler latches the bits and hands each to one
//! CHIMI_REQUEST_SIGNAL_WAIT. The handlers are installed when the program's
//! `signals` provider is built, before main's body, and never removed, so a
//! program that does not need `signals` keeps the default actions (SIGHUP
//! ends it).
//!
//! Child processes for binding packages (std/exec, CHI-250): SIGCHLD's
//! handler only writes to the stop signals' pipe, which wakes an idle
//! scheduler; CHIMI_REQUEST_CHILD_WAIT then checks every waiting child.
//!
//! Every handler is async-signal-safe: it touches only atomics, `errno`
//! and `write(2)`, and cannot panic.

use core::ffi::c_int;
use core::ptr;
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, Ordering};

use super::standard::{defect, shape_defect, success, void_value};
use crate::heap::layout::{Control, ControlKind, Intrinsic, Runtime, Scheduler, SchedulerRequest, Value, ValueKind};
use crate::heap::list;
use crate::heap::values::{integer_value, new_control};
use crate::scheduler;

/// The first stop signal's number, 0 before one arrives. The scheduler
/// reads it through [`chimi_runtime_stop_signal_v1`].
static RECEIVED: AtomicI32 = AtomicI32::new(0);
/// The pipe a handler writes a byte to, to wake the scheduler's reactor;
/// -1 without the scheduler.
static PIPE_READ: AtomicI32 = AtomicI32::new(-1);
static PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);
/// The runtime whose `cancel` a stop signal sets, when there is no
/// scheduler to deliver it.
static CANCEL_RUNTIME: AtomicPtr<Runtime> = AtomicPtr::new(ptr::null_mut());
/// std/signal's received signals, bit `code - 1` each, which the scheduler
/// latches and clears.
static USER_SIGNALS: AtomicU32 = AtomicU32::new(0);
/// SIGHUP, SIGUSR1 and SIGUSR2: std/signal's codes 1 to 3.
const USER_SIGNAL_NUMBERS: [c_int; 3] = [libc::SIGHUP, libc::SIGUSR1, libc::SIGUSR2];

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

/// Writes one byte to the wake pipe, keeping the interrupted code's `errno`.
fn wake_scheduler() {
    let fd = PIPE_WRITE.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    static BYTE: u8 = 0;
    // SAFETY: errno's location is the thread's; the byte is readable. A
    // full pipe already holds a wake, so a failed write loses nothing.
    unsafe {
        let saved = *errno_location();
        libc::write(fd, (&raw const BYTE).cast(), 1);
        *errno_location() = saved;
    }
}

/// Ends the program by `number`'s default action. Dying by the signal
/// tells a parent (a shell, systemd, Kubernetes) that the program stopped
/// because it was asked to. As process 1 in a container a default-action
/// signal is ignored, so it falls back to the shell's status.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_end_by_signal_v1(number: c_int) -> ! {
    // SAFETY: each call is async-signal-safe and takes valid arguments.
    unsafe {
        libc::signal(number, libc::SIG_DFL);
        // Inside the handler the signal is blocked; unblock it so it lands now.
        let mut unblock: libc::sigset_t = core::mem::zeroed();
        libc::sigemptyset(&mut unblock);
        libc::sigaddset(&mut unblock, number);
        libc::sigprocmask(libc::SIG_UNBLOCK, &unblock, ptr::null_mut());
        libc::raise(number);
        libc::_exit(128 + number)
    }
}

extern "C" fn on_stop_signal(number: c_int) {
    if RECEIVED.load(Ordering::Relaxed) != 0 {
        chimi_runtime_end_by_signal_v1(number);
    }
    RECEIVED.store(number, Ordering::Relaxed);
    wake_scheduler();
    let runtime = CANCEL_RUNTIME.load(Ordering::Relaxed);
    if !runtime.is_null() {
        // SAFETY: the runtime outlives the program's handlers; the flag is
        // read volatile by the host operations that poll it.
        unsafe { ptr::write_volatile(&raw mut (*runtime).cancel, 1) };
    }
}

extern "C" fn on_user_signal(number: c_int) {
    for (bit, &candidate) in USER_SIGNAL_NUMBERS.iter().enumerate() {
        if candidate == number {
            USER_SIGNALS.fetch_or(1 << bit, Ordering::SeqCst);
        }
    }
    wake_scheduler();
}

extern "C" fn on_child_signal(_number: c_int) {
    wake_scheduler();
}

/// Installs `handler` for each of `numbers`, with every one of them
/// blocked while it runs.
fn install(handler: extern "C" fn(c_int), numbers: &[c_int], flags: c_int) {
    // SAFETY: `action` is fully initialized before sigaction reads it.
    unsafe {
        let mut action: libc::sigaction = core::mem::zeroed();
        action.sa_sigaction = handler as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        for &number in numbers {
            libc::sigaddset(&mut action.sa_mask, number);
        }
        action.sa_flags = flags;
        for &number in numbers {
            libc::sigaction(number, &action, ptr::null_mut());
        }
    }
}

/// Moves `descriptor` above standard error and makes it non-blocking and
/// closed on exec; false when a call fails.
fn prepare_descriptor(descriptor: &mut c_int) -> bool {
    // SAFETY: fcntl and close take any descriptor.
    unsafe {
        if *descriptor <= libc::STDERR_FILENO {
            let moved = libc::fcntl(*descriptor, libc::F_DUPFD, libc::STDERR_FILENO + 1);
            if moved < 0 {
                return false;
            }
            libc::close(*descriptor);
            *descriptor = moved;
        }
        libc::fcntl(*descriptor, libc::F_SETFL, libc::O_NONBLOCK) == 0
            && libc::fcntl(*descriptor, libc::F_SETFD, libc::FD_CLOEXEC) == 0
    }
}

/// Installs the SIGINT and SIGTERM handlers. A program on the scheduler
/// gets the wake pipe first, and its interrupted reads restart, since the
/// scheduler sees a signal between steps; without it, a read returns EINTR
/// so the host operation sees `runtime`'s `cancel`. False when the pipe
/// cannot be made.
///
/// # Safety
/// `runtime` lives until the process exits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_install_stop_signals_v1(runtime: *mut Runtime) -> bool {
    let scheduled = scheduler::requires_scheduler();
    if scheduled {
        let mut pipe: [c_int; 2] = [-1, -1];
        // SAFETY: `pipe` is two writable descriptors.
        if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
            return false;
        }
        let [mut read, mut write] = pipe;
        if !prepare_descriptor(&mut read) || !prepare_descriptor(&mut write) {
            return false;
        }
        PIPE_READ.store(read, Ordering::Relaxed);
        PIPE_WRITE.store(write, Ordering::Relaxed);
    } else {
        CANCEL_RUNTIME.store(runtime, Ordering::Relaxed);
    }
    install(on_stop_signal, &[libc::SIGINT, libc::SIGTERM], if scheduled { libc::SA_RESTART } else { 0 });
    true
}

/// The first stop signal's number, 0 before one arrives, for the
/// scheduler's configuration and the exit status.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_stop_signal_v1() -> *mut i32 {
    RECEIVED.as_ptr()
}

/// The wake pipe's read end, -1 without the scheduler.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_signal_fd_v1() -> c_int {
    PIPE_READ.load(Ordering::Relaxed)
}

/// std/signal's received bits, which the scheduler latches and clears.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_user_signals_v1() -> *mut u32 {
    USER_SIGNALS.as_ptr()
}

/// Runs `install` once per process, from whichever thread first asks: a
/// second caller returns only after the handlers are in place.
fn once(control: &'static Once, install: extern "C" fn()) {
    // libc declares the routine as a bare function on Linux and as an
    // `Option` on Apple.
    #[cfg(target_vendor = "apple")]
    let install = Some(install as unsafe extern "C" fn());
    // SAFETY: the control lives for the process; `install` takes nothing.
    unsafe { libc::pthread_once(control.0.get(), install) };
}

struct Once(core::cell::UnsafeCell<libc::pthread_once_t>);

// SAFETY: pthread_once synchronizes every access to the control.
unsafe impl Sync for Once {}

static USER_SIGNALS_INSTALLED: Once = Once(core::cell::UnsafeCell::new(libc::PTHREAD_ONCE_INIT));
static CHILD_SIGNAL_INSTALLED: Once = Once(core::cell::UnsafeCell::new(libc::PTHREAD_ONCE_INIT));

extern "C" fn install_user_signals() {
    install(on_user_signal, &USER_SIGNAL_NUMBERS, libc::SA_RESTART);
}

/// Installed by the first child start, before the spawn: an inherited
/// SIG_IGN would make the kernel reap children on its own.
extern "C" fn install_child_signal() {
    install(on_child_signal, &[libc::SIGCHLD], libc::SA_RESTART | libc::SA_NOCLDSTOP);
}

/// chimi_binding.h: installs the SIGCHLD handler, once, so a child's exit
/// wakes the scheduler.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_binding_prepare_children() {
    once(&CHILD_SIGNAL_INSTALLED, install_child_signal);
}

/// `Signal.host_install()` and `Signal.host_wait(codes)`. The wait passes
/// the distinct codes in their listed order, one byte each, and returns the
/// one the scheduler hands it. Programs that reach either always run on the
/// scheduler.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live values.
#[inline(never)]
pub unsafe fn signal_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if intrinsic == Intrinsic::SIGNAL_HOST_INSTALL {
            if !arguments.is_empty() {
                return shape_defect(call_node);
            }
            once(&USER_SIGNALS_INSTALLED, install_user_signals);
            return success(void_value());
        }
        let &[codes] = arguments else { return shape_defect(call_node) };
        if (*codes).kind != ValueKind::LIST || !scheduler::requires_scheduler() {
            return shape_defect(call_node);
        }
        let mut packed: u64 = 0;
        let mut count: u32 = 0;
        for index in 0..(*codes).payload.list.length {
            let code = list::get(codes, index);
            if (*code).kind != ValueKind::INTEGER || !(1..=3).contains(&(*code).payload.integer) {
                return shape_defect(call_node);
            }
            let code = (*code).payload.integer as u64;
            let seen = (0..count).any(|earlier| (packed >> (8 * earlier)) & 0xff == code);
            if !seen {
                packed |= code << (8 * count);
                count += 1;
            }
        }
        if count == 0 {
            return defect(c"Signal.next needs at least one signal to wait for", i64::from(call_node));
        }
        loop {
            let mut request = SchedulerRequest::of(Scheduler::REQUEST_SIGNAL_WAIT);
            request.value = packed as i64;
            if scheduler::request(&mut request) {
                return new_control(ControlKind::CANCEL, ptr::null_mut(), ptr::null(), -1);
            }
            match request.status {
                Scheduler::RESPONSE_OK => return success(integer_value(ValueKind::INTEGER, request.value)),
                Scheduler::RESPONSE_PENDING => {}
                _ => return shape_defect(call_node),
            }
        }
    }
}
