//! Console output (CHI-266): `write_line`, `write_error` and std/log's host
//! core, which write standard output, standard error and `--log-file`
//! (streams 0, 1 and 2).
//!
//! A writer on a pipe, terminal or socket queues its line and parks until
//! the line is written instead of blocking its carrier while the reader is
//! slow, and the queue is capped at [`QUEUE_LIMIT`] bytes: a writer waits
//! before it queues a line that would pass the limit, so writers cancelled
//! while they wait (a write_line under Task.timeout against a stalled reader)
//! cannot grow it without bound. A line longer than the limit is queued when
//! the queue is empty. The queues are [`ConsoleQueue`]s.
//!
//! The descriptor stays blocking, because its open file description is
//! shared with the parent shell and other processes; after poll reports it
//! writable, one write(2) of at most PIPE_BUF bytes does not block on a pipe
//! or a socket (a terminal may still hold one briefly), and a socket is
//! written with send(MSG_DONTWAIT). Lines from different tasks never
//! interleave: each line goes whole onto its stream's queue before its
//! writer waits, and whichever writer the reactor wakes writes the front of
//! the queue, its own bytes or an earlier line's. A line once queued is
//! written whole, even when its writer is cancelled while it waits: later
//! writers or the end of the program write it.
//!
//! A regular file, a program without the scheduler, and a program under the
//! virtual clock write directly: a regular file is always writable, without
//! the scheduler there is no other task to keep running, and under the
//! virtual clock the order tasks run in must not depend on how fast the
//! reader drains the pipe.
//!
//! One lock guards the streams and queues, since a task on any carrier
//! writes lines. A writer holds it except while it parks. The drain at exit
//! may start inside a writer that holds it (an exhausted allocation), so the
//! holder is noted per carrier (`Carrier.console_lock_held`). With one
//! carrier there is no other writer, so a line takes no lock: the lock
//! measured about 25 ns a line on a program that only prints.

use core::cell::UnsafeCell;
use core::ffi::{CStr, c_int};
use core::ptr;

use super::standard::{defect, failure, shape_defect, standard_value, success, void_value};
use crate::console::{ConsoleQueue, LineState, log_line};
use crate::heap::allocator::{carrier_count, current_carrier};
use crate::heap::layout::{
    Control, ControlKind, Core, Intrinsic, Runtime, Scheduler, SchedulerRequest, StandardConstructor, Value,
    ValueKind,
};
use crate::heap::values::{integer_value, new_control, text_value};
use crate::scheduler;

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
    fn abort() -> !;
}

/// The bytes a queue may hold before a writer waits to add more.
const QUEUE_LIMIT: usize = 1 << 20;
const STREAMS: usize = 3;
const LOG_STREAM: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Unknown,
    Direct,
    Queued,
}

struct Stream {
    fd: c_int,
    mode: Mode,
    socket: bool,
    /// Its own queue, or an earlier stream's when both descriptors name the
    /// same open file (`2>&1`, one terminal), so their lines never
    /// interleave.
    queue: usize,
}

struct Console {
    streams: [Stream; STREAMS],
    queues: [ConsoleQueue; STREAMS],
}

struct Shared {
    lock: UnsafeCell<libc::pthread_mutex_t>,
    console: UnsafeCell<Console>,
}

// SAFETY: `console` is reached only by the lock's holder, or with one
// carrier, where nothing else runs.
unsafe impl Sync for Shared {}

static SHARED: Shared = Shared {
    lock: UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER),
    console: UnsafeCell::new(Console {
        streams: [
            Stream { fd: libc::STDOUT_FILENO, mode: Mode::Unknown, socket: false, queue: 0 },
            Stream { fd: libc::STDERR_FILENO, mode: Mode::Unknown, socket: false, queue: 1 },
            Stream { fd: -1, mode: Mode::Unknown, socket: false, queue: 2 },
        ],
        queues: [ConsoleQueue::new(), ConsoleQueue::new(), ConsoleQueue::new()],
    }),
};

/// The console state. ThreadSanitizer cannot see inside this crate, so a
/// caller that forgot the lock would race unseen: this aborts instead
/// unless this carrier holds the lock or one carrier runs.
///
/// # Safety
/// The caller holds the console lock (see [`enter`]), and drops the
/// reference before it leaves.
unsafe fn state<'a>() -> &'a mut Console {
    // SAFETY: this thread's carrier is live.
    if carrier_count() != 1 && !unsafe { (*current_carrier()).console_lock_held } {
        // SAFETY: no arguments; never returns.
        unsafe { abort() }
    }
    // SAFETY: the caller's contract.
    unsafe { &mut *SHARED.console.get() }
}

/// Takes the console lock, unless one carrier runs (its count changes
/// only before carrier threads start).
fn enter() {
    if carrier_count() == 1 {
        return;
    }
    // SAFETY: the mutex is statically initialised and never moves.
    if unsafe { libc::pthread_mutex_lock(SHARED.lock.get()) } != 0 {
        // SAFETY: no arguments; never returns.
        unsafe { abort() }
    }
    // SAFETY: this thread's carrier is live.
    unsafe { (*current_carrier()).console_lock_held = true };
}

fn leave() {
    if carrier_count() == 1 {
        return;
    }
    // SAFETY: this thread's carrier is live, and this thread holds the lock.
    unsafe {
        (*current_carrier()).console_lock_held = false;
        libc::pthread_mutex_unlock(SHARED.lock.get());
    }
}

fn errno() -> c_int {
    // SAFETY: the thread's errno location is always valid.
    unsafe { *errno_location() }
}

#[cfg(target_os = "linux")]
unsafe fn errno_location() -> *mut c_int {
    // SAFETY: no preconditions.
    unsafe { libc::__errno_location() }
}

#[cfg(target_os = "macos")]
unsafe fn errno_location() -> *mut c_int {
    // SAFETY: no preconditions.
    unsafe { libc::__error() }
}

fn status_of(fd: c_int) -> Option<libc::stat> {
    let mut status = core::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat writes the whole struct when it succeeds.
    (unsafe { libc::fstat(fd, status.as_mut_ptr()) } == 0).then(|| unsafe { status.assume_init() })
}

fn is_kind(status: &libc::stat, kind: libc::mode_t) -> bool {
    status.st_mode & libc::S_IFMT == kind
}

/// How stream `index` is written, decided on its first line.
fn mode(console: &mut Console, virtual_clock: bool, index: usize) -> Mode {
    let stream = &mut console.streams[index];
    if stream.mode != Mode::Unknown {
        return stream.mode;
    }
    stream.queue = index;
    let status = status_of(stream.fd);
    stream.socket = status.as_ref().is_some_and(|status| is_kind(status, libc::S_IFSOCK));
    let queued = scheduler::requires_scheduler()
        && !virtual_clock
        && status.as_ref().is_some_and(|status| !is_kind(status, libc::S_IFREG));
    stream.mode = if queued { Mode::Queued } else { Mode::Direct };
    if let (true, Some(status)) = (queued, status) {
        for earlier in 0..index {
            if console.streams[earlier].fd >= 0
                && mode(console, virtual_clock, earlier) == Mode::Queued
                && status_of(console.streams[earlier].fd)
                    .is_some_and(|other| other.st_dev == status.st_dev && other.st_ino == status.st_ino)
            {
                console.streams[index].queue = console.streams[earlier].queue;
                break;
            }
        }
    }
    console.streams[index].mode
}

/// Writes `text` and a newline straight to `fd`. 0, or the errno of the
/// failed write. The newline goes out even after a failed write, so a later
/// line never joins a broken one, unless a signal interrupted it.
fn write_direct(fd: c_int, text: &[u8]) -> c_int {
    let newline = *b"\n";
    let (mut body, mut tail): (&[u8], &[u8]) = (text, &newline);
    let mut error = 0;
    while !body.is_empty() || !tail.is_empty() {
        let parts = [
            libc::iovec { iov_base: body.as_ptr().cast_mut().cast(), iov_len: body.len() },
            libc::iovec { iov_base: tail.as_ptr().cast_mut().cast(), iov_len: tail.len() },
        ];
        let from = usize::from(body.is_empty());
        // SAFETY: both parts name live, readable bytes.
        let count = unsafe { libc::writev(fd, parts[from..].as_ptr(), (2 - from) as c_int) };
        let Ok(mut count) = usize::try_from(count) else {
            let failed = errno();
            // Without the scheduler a stop signal interrupts the write (no
            // SA_RESTART), so the line fails and the program can end.
            if failed == libc::EINTR {
                return libc::EINTR;
            }
            if error == 0 {
                error = if failed == 0 { libc::EIO } else { failed };
            }
            if body.is_empty() {
                break;
            }
            body = &[];
            continue;
        };
        let taken = count.min(body.len());
        body = &body[taken..];
        count -= taken;
        tail = &tail[count.min(tail.len())..];
    }
    error
}

/// One write(2) from the front of queue `stream.queue`, at most `limit`
/// bytes. False with errno set when it failed; an interruption or a
/// descriptor a parent left non-blocking reports nothing written and true.
fn write_front(console: &mut Console, index: usize, limit: usize) -> bool {
    let stream = &console.streams[index];
    let queue = &mut console.queues[stream.queue];
    let front = queue.front();
    let length = front.len().min(limit);
    // SAFETY: `front` holds at least `length` readable bytes.
    let count = unsafe {
        if stream.socket {
            libc::send(stream.fd, front.as_ptr().cast(), length, libc::MSG_DONTWAIT)
        } else {
            libc::write(stream.fd, front.as_ptr().cast(), length)
        }
    };
    match usize::try_from(count) {
        Ok(count) => {
            queue.advance(count);
            true
        }
        Err(_) => matches!(errno(), libc::EINTR | libc::EAGAIN) || errno() == libc::EWOULDBLOCK,
    }
}

/// Writes the whole queue of stream `index`, blocking this thread. Only the
/// end of the program uses it, when no task is left to keep running.
fn drain_blocking(console: &mut Console, index: usize) {
    let (fd, queue) = (console.streams[index].fd, console.streams[index].queue);
    while console.queues[queue].queued() != 0 {
        let mut poll = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
        // SAFETY: one live pollfd.
        if unsafe { libc::poll(&mut poll, 1, -1) } < 0 && errno() != libc::EINTR {
            console.queues[queue].fail(errno());
            return;
        }
        if !write_front(console, index, usize::MAX) {
            console.queues[queue].fail(errno());
            return;
        }
    }
}

/// Writes every queued line, blocking: `runtime.c` calls it at exit and
/// before its reports.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_console_drain_v1() {
    // SAFETY: this thread's carrier is live.
    let entered = !unsafe { (*current_carrier()).console_lock_held };
    if entered {
        enter();
    }
    // SAFETY: the lock is held, or one carrier runs.
    let console = unsafe { state() };
    for index in 0..STREAMS {
        if console.streams[index].mode == Mode::Queued {
            drain_blocking(console, index);
        }
    }
    if entered {
        leave();
    }
}

/// Points stream `index` at `fd` (-1 for none). `direct` writes it straight
/// away, as a regular file is; otherwise its first line decides. Called
/// before carrier threads start, or after they stop.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_runtime_console_set_v1(index: usize, fd: c_int, direct: bool) {
    // SAFETY: no writer runs.
    let console = unsafe { &mut *SHARED.console.get() };
    if let Some(stream) = console.streams.get_mut(index) {
        stream.fd = fd;
        stream.mode = if direct { Mode::Direct } else { Mode::Unknown };
        stream.queue = index;
    }
}

enum Waited {
    Again,
    Cancelled,
}

/// Parks until stream `index` is writable and writes the front of its
/// queue. When the reactor cannot wait on the descriptor (it is closed, or
/// poll failed) the write fails, as a read of "-" does: writing anyway could
/// block the scheduler thread. The caller holds the lock, which is never
/// held across a park.
fn wait_and_write(index: usize) -> Waited {
    // SAFETY: the caller holds the lock.
    let fd = unsafe { state() }.streams[index].fd;
    let mut request = SchedulerRequest::of(Scheduler::REQUEST_IO_WAIT);
    request.value = i64::from(fd);
    request.token = Scheduler::IO_WRITABLE;
    leave();
    // SAFETY: a queued stream implies a program that starts the scheduler,
    // running on a fiber.
    let cancel = unsafe { scheduler::request(&mut request) };
    enter();
    if cancel {
        return Waited::Cancelled;
    }
    // SAFETY: the lock is held again.
    let console = unsafe { state() };
    let queue = console.streams[index].queue;
    if request.status == Scheduler::RESPONSE_PENDING {
        return Waited::Again;
    }
    if request.status == Scheduler::RESPONSE_ERROR {
        // SAFETY: F_GETFD only asks.
        let closed = unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0;
        console.queues[queue].fail(if closed { libc::EBADF } else { libc::EIO });
        return Waited::Again;
    }
    // Another writer may have emptied the queue while this one waited.
    if console.queues[queue].queued() != 0 && !write_front(console, index, libc::PIPE_BUF) {
        console.queues[queue].fail(errno());
    }
    Waited::Again
}

fn push(queue: &mut ConsoleQueue, bytes: &[u8]) {
    if !queue.push(bytes) {
        // SAFETY: no arguments; never returns.
        unsafe { chimi_runtime_exhausted_memory() }
    }
}

/// Writes `text` and a newline to stream `index`. 0 when written (or, for a
/// body ending under a terminal cancel, queued), -1 when cancelled,
/// otherwise the errno of the failed write. The caller holds the lock.
fn write_line_locked(virtual_clock: bool, index: usize, text: &[u8]) -> c_int {
    let queue = {
        // SAFETY: the caller holds the lock; the borrow ends before any wait.
        let console = unsafe { state() };
        if mode(console, virtual_clock, index) == Mode::Direct {
            return write_direct(console.streams[index].fd, text);
        }
        console.streams[index].queue
    };
    // A body ending under a terminal cancel must not park (CHI-265), and
    // must not block the other tasks either: its line is queued for later
    // writers or the end of the program, past the limit if need be.
    let terminal = scheduler::cancel_is_terminal();
    // Nothing is queued yet, so a cancellation here loses nothing.
    while !terminal && {
        // SAFETY: the lock is held (again, after a wait).
        let queued = unsafe { state() }.queues[queue].queued();
        queued != 0 && queued + text.len() + 1 > QUEUE_LIMIT
    } {
        if let Waited::Cancelled = wait_and_write(index) {
            return -1;
        }
    }
    // SAFETY: the lock is held.
    let lines = &mut unsafe { state() }.queues[queue];
    push(lines, text);
    push(lines, b"\n");
    if terminal {
        return 0;
    }
    let Some(line) = lines.begin_line() else {
        // SAFETY: no arguments; never returns.
        unsafe { chimi_runtime_exhausted_memory() }
    };
    let outcome = loop {
        // SAFETY: the lock is held (again, after a wait).
        match unsafe { state() }.queues[queue].line_state(line) {
            LineState::Written => break 0,
            LineState::Failed(error) => break error,
            LineState::Pending => {
                if let Waited::Cancelled = wait_and_write(index) {
                    break -1;
                }
            }
        }
    };
    // SAFETY: the lock is held.
    unsafe { state() }.queues[queue].end_line(line);
    outcome
}

/// Writes `text` and a newline to stream `index`: 0, -1 when cancelled, or
/// the errno of the failed write.
///
/// # Safety
/// `runtime` is the live runtime; runs on a fiber when the program starts
/// the scheduler.
unsafe fn write_line(runtime: *const Runtime, index: usize, text: &[u8]) -> c_int {
    // SAFETY: the caller's contract.
    let virtual_clock = unsafe { (*runtime).virtual_clock };
    enter();
    let outcome = write_line_locked(virtual_clock, index, text);
    leave();
    outcome
}

/// [`write_line`] for the carrier stress harness, whose threads write lines
/// without the scheduler.
///
/// # Safety
/// `runtime` is live and `text` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_console_write_line_v1(
    runtime: *const Runtime,
    index: usize,
    text: *const u8,
    length: usize,
) -> c_int {
    if index >= STREAMS {
        return libc::EBADF;
    }
    // SAFETY: the caller's contract.
    unsafe { write_line(runtime, index, core::slice::from_raw_parts(text, length)) }
}

/// The bytes of a Text value.
///
/// # Safety
/// `text` is a live Text.
unsafe fn text_bytes<'a>(text: *const Value) -> &'a [u8] {
    // SAFETY: the caller's contract; a Text's bytes are never null.
    unsafe {
        let payload = (*text).payload.text;
        core::slice::from_raw_parts(payload.bytes.cast::<u8>(), payload.length)
    }
}

/// The running task was cancelled.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn cancelled() -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { new_control(ControlKind::CANCEL, ptr::null_mut(), ptr::null(), -1) }
}

/// # Safety
/// `runtime` is live.
unsafe fn is_cancelled(runtime: *const Runtime) -> bool {
    // SAFETY: the caller's contract; another carrier may set the flag.
    unsafe { ptr::read_volatile(&raw const (*runtime).cancel) != 0 }
}

/// `write_line` and `write_error`, the console need's host functions.
///
/// # Safety
/// Runs on a fiber that may allocate; `runtime` and the arguments are live.
#[inline(never)]
pub unsafe fn console_host(
    runtime: *mut Runtime,
    function: u32,
    arguments: &[*mut Value],
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let [text] = arguments else {
            return defect(c"verified host function has invalid runtime arguments", i64::from(call_node));
        };
        if (**text).kind != ValueKind::TEXT {
            return defect(c"verified host function has invalid runtime arguments", i64::from(call_node));
        }
        if is_cancelled(runtime) {
            return cancelled();
        }
        let index = usize::from(function != Core::HOST_WRITE_LINE);
        let written = write_line(runtime, index, text_bytes(*text));
        if written < 0 {
            return cancelled();
        }
        if written > 0 {
            let name = NAMES[index];
            let fields = [
                text_value(name.as_ptr(), name.count_bytes()),
                text_value(libc::strerror(written), libc::strlen(libc::strerror(written))),
            ];
            return failure(standard_value(StandardConstructor::CONSOLE_WRITE_FAILED, &fields));
        }
        if is_cancelled(runtime) { cancelled() } else { success(void_value()) }
    }
}

/// The stream a failed `write_line` or `write_error` names.
const NAMES: [&CStr; 2] = [c"stdout", c"stderr"];

/// std/log's host core. `host_write` gets the message and fields already
/// encoded as JSON and writes one line whole: to the `--log-file` file, or
/// to standard error through its queue, so log lines and the program's own
/// write_error lines never interleave. A line that cannot be written is
/// lost: logging never fails.
///
/// # Safety
/// Runs on a fiber that may allocate; `runtime` and the arguments are live.
#[inline(never)]
pub unsafe fn log_operation(
    runtime: *mut Runtime,
    intrinsic: u32,
    arguments: &[*mut Value],
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if intrinsic == Intrinsic::LOG_HOST_MINIMUM_LEVEL {
            if !arguments.is_empty() {
                return shape_defect(call_node);
            }
            return success(integer_value(ValueKind::INTEGER, (*runtime).log_level));
        }
        let [level, message, fields] = arguments else { return shape_defect(call_node) };
        if (**level).kind != ValueKind::INTEGER
            || (**message).kind != ValueKind::TEXT
            || (**fields).kind != ValueKind::TEXT
        {
            return shape_defect(call_node);
        }
        if is_cancelled(runtime) {
            return cancelled();
        }
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut now);
        let Some(line) = log_line(
            now.tv_sec,
            now.tv_nsec / 1_000_000,
            (**level).payload.integer,
            text_bytes(*message),
            text_bytes(*fields),
        ) else {
            chimi_runtime_exhausted_memory()
        };
        let index = if (*runtime).log_file.is_null() { 1 } else { LOG_STREAM };
        let written = write_line(runtime, index, &line);
        drop(line);
        if written < 0 || is_cancelled(runtime) { cancelled() } else { success(void_value()) }
    }
}
