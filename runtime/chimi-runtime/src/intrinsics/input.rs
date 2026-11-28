//! `Stream.process_utf8_lines`, the line reader of std/io over standard
//! input ("-") or a file, and the wait a blocked std/fs read or write makes.
//!
//! Standard input's buffer is process-wide, so the bytes a read takes past an
//! early exit (a failing callback, a cancellation) wait for the next read of
//! "-", as stdio's buffer kept them. Tasks on any carrier may read "-", so the
//! buffer has a lock. A reader holds it while it takes a line's bytes and lets
//! go before each callback and while it waits for more input, so, as with one
//! carrier, two readers of "-" may split a line that arrives in pieces; it
//! keeps the buffer whole, not the lines.

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::mem::MaybeUninit;
use core::ptr;

use super::clocks::ClockCall;
use super::standard::{defect, failure, standard_value, success};
use crate::eval::end_measured_scope;
use crate::heap::allocator::{allocate, current_carrier};
use crate::heap::collector;
use crate::heap::layout::{
    AllocationScope, Callable, Control, ControlKind, Runtime, Scheduler, SchedulerRequest, StandardConstructor,
    Value, ValueKind,
};
use crate::heap::values::{integer_value, new_control, text_value};
use crate::heap::young;
use crate::scheduler;

unsafe extern "C" {
    fn abort() -> !;
}

const INPUT_BUFFER_BYTES: usize = 64 * 1024;

/// Bytes read from a descriptor and not yet consumed.
struct ByteInput {
    fd: c_int,
    bytes: *mut u8,
    start: usize,
    end: usize,
    /// Standard input's lock; null for a file one call reads.
    lock: *mut libc::pthread_mutex_t,
}

struct StandardInput {
    lock: UnsafeCell<libc::pthread_mutex_t>,
    input: UnsafeCell<ByteInput>,
}

/// Standard input's buffer, a static of its own: all zero, it stays out of
/// the program file (in `.bss`), which it would not inside
/// [`STANDARD_INPUT`], whose pointers make that static initialized data.
struct InputBytes(UnsafeCell<[u8; INPUT_BUFFER_BYTES]>);

// SAFETY: `input` and the buffer are reached only by the lock's holder.
unsafe impl Sync for StandardInput {}
// SAFETY: as for `StandardInput`, whose lock guards it.
unsafe impl Sync for InputBytes {}

static STANDARD_INPUT_BYTES: InputBytes = InputBytes(UnsafeCell::new([0; INPUT_BUFFER_BYTES]));

static STANDARD_INPUT: StandardInput = StandardInput {
    lock: UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER),
    input: UnsafeCell::new(ByteInput {
        fd: libc::STDIN_FILENO,
        bytes: STANDARD_INPUT_BYTES.0.get().cast(),
        start: 0,
        end: 0,
        lock: STANDARD_INPUT.lock.get(),
    }),
};

/// # Safety
/// `input` is standard input's, or a file's that this thread owns.
unsafe fn lock(input: *mut ByteInput) {
    // SAFETY: the caller's contract; the mutex is static and never moves.
    unsafe {
        if !(*input).lock.is_null() && libc::pthread_mutex_lock((*input).lock) != 0 {
            abort()
        }
    }
}

/// # Safety
/// This thread holds `input`'s lock.
unsafe fn unlock(input: *mut ByteInput) {
    // SAFETY: the caller's contract.
    unsafe {
        if !(*input).lock.is_null() {
            libc::pthread_mutex_unlock((*input).lock);
        }
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

/// # Safety
/// `runtime` is live.
unsafe fn is_cancelled(runtime: *const Runtime) -> bool {
    // SAFETY: the caller's contract; another carrier may set the flag.
    unsafe { ptr::read_volatile(&raw const (*runtime).cancel) != 0 }
}

enum Waited {
    Ready,
    Cancelled,
    /// The reactor cannot wait on the descriptor (it is closed, or poll
    /// failed): a read then could block the scheduler thread.
    Failed(c_int),
}

/// Waits until `input` may be read. Under the scheduler the task parks in
/// CHIMI_REQUEST_IO_WAIT, so other tasks run and a signal is delivered
/// meanwhile; its OK reply resumes this task before any other, so the read
/// that follows does not block unless another OS process drained the
/// descriptor first. It also ends when another task reading "-" has refilled
/// the shared buffer meanwhile. Without the scheduler a signal interrupts
/// poll with EINTR and sets `cancel`, which the caller checks.
///
/// # Safety
/// The caller holds `input`'s lock, which is let go while it waits.
unsafe fn wait_readable(input: *mut ByteInput) -> Waited {
    // SAFETY: the caller's contract.
    unsafe {
        let fd = (*input).fd;
        if scheduler::requires_scheduler() {
            loop {
                if (*input).start < (*input).end {
                    return Waited::Ready;
                }
                let mut request = SchedulerRequest::of(Scheduler::REQUEST_IO_WAIT);
                request.value = i64::from(fd);
                request.token = Scheduler::IO_READABLE;
                unlock(input);
                let cancel = scheduler::request(&mut request);
                lock(input);
                if cancel {
                    return Waited::Cancelled;
                }
                if request.status == Scheduler::RESPONSE_ERROR {
                    let closed = libc::fcntl(fd, libc::F_GETFD) < 0;
                    return Waited::Failed(if closed { libc::EBADF } else { libc::EIO });
                }
                if request.status != Scheduler::RESPONSE_PENDING {
                    return Waited::Ready;
                }
            }
        }
        let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        unlock(input);
        libc::poll(&mut poll, 1, -1);
        lock(input);
        Waited::Ready
    }
}

enum Next {
    Byte(u8),
    End,
    Cancelled,
    Failed(c_int),
}

/// The next byte of `input`.
///
/// # Safety
/// The caller holds `input`'s lock; a wait lets go of it meanwhile.
unsafe fn next(runtime: *const Runtime, input: *mut ByteInput) -> Next {
    // SAFETY: the caller's contract; `bytes` holds INPUT_BUFFER_BYTES.
    unsafe {
        let take = |input: *mut ByteInput| {
            let byte = *(*input).bytes.add((*input).start);
            (*input).start += 1;
            Next::Byte(byte)
        };
        if (*input).start < (*input).end {
            return take(input);
        }
        let scheduled = scheduler::requires_scheduler();
        loop {
            if is_cancelled(runtime) {
                return Next::Cancelled;
            }
            if scheduled {
                match wait_readable(input) {
                    Waited::Cancelled => return Next::Cancelled,
                    Waited::Failed(error) => return Next::Failed(error),
                    Waited::Ready => {}
                }
                // Another task reading "-" may have refilled the shared
                // buffer while this one waited; reading over it would lose
                // those bytes.
                if (*input).start < (*input).end {
                    return take(input);
                }
            }
            let count = libc::read((*input).fd, (*input).bytes.cast::<c_void>(), INPUT_BUFFER_BYTES);
            if count > 0 {
                (*input).start = 0;
                (*input).end = count as usize;
                return take(input);
            }
            if count == 0 {
                return Next::End;
            }
            let error = errno();
            if error == libc::EINTR {
                continue;
            }
            // A descriptor a parent left non-blocking: wait for it, then
            // read again.
            if error == libc::EAGAIN || error == libc::EWOULDBLOCK {
                if !scheduled {
                    wait_readable(input);
                }
                continue;
            }
            return Next::Failed(error);
        }
    }
}

/// Reads one line of standard input into `out` for the carrier stress
/// harness, as `process_utf8_lines` takes its bytes: under the lock, which a
/// wait lets go. Returns the line's length, of which the first `capacity`
/// bytes are stored, or -1 at the end of the input and -2 when the read
/// failed, was cancelled or ended mid-line.
///
/// # Safety
/// `runtime` is live and `out` holds `capacity` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_input_line_v1(runtime: *const Runtime, out: *mut u8, capacity: usize) -> i64 {
    let input = STANDARD_INPUT.input.get();
    // SAFETY: the caller's contract.
    unsafe {
        lock(input);
        let mut length = 0usize;
        let ended = loop {
            match next(runtime, input) {
                Next::Byte(b'\n') => break None,
                Next::Byte(byte) => {
                    if length < capacity {
                        *out.add(length) = byte;
                    }
                    length += 1;
                }
                Next::End if length == 0 => break Some(-1),
                Next::End | Next::Cancelled | Next::Failed(_) => break Some(-2),
            }
        };
        unlock(input);
        ended.unwrap_or(length as i64)
    }
}

/// Whether `bytes` are UTF-8 without U+0000, which a line may not hold.
fn valid_line(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && core::str::from_utf8(bytes).is_ok()
}

/// A failure `constructor(path, message)`.
///
/// # Safety
/// Runs on a fiber that may allocate; `path` is a live Text.
unsafe fn file_message(constructor: u32, path: *mut Value, message: *const c_char) -> *mut Control {
    // SAFETY: the caller's contract; strerror's text is NUL-terminated.
    unsafe {
        let fields = [
            text_value((*path).payload.text.bytes, (*path).payload.text.length),
            text_value(message, libc::strlen(message)),
        ];
        failure(standard_value(constructor, &fields))
    }
}

/// Calls the line callback with `state` and the line, or fails with
/// `Utf8Invalid(path, line)` when the line is not text.
///
/// # Safety
/// Runs on a fiber that may allocate; the values are live.
unsafe fn line_callback(
    call: ClockCall,
    callback: *mut Callable,
    state: *mut Value,
    bytes: &[u8],
    line: u64,
    path: *mut Value,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if !valid_line(bytes) {
            let fields = [
                text_value((*path).payload.text.bytes, (*path).payload.text.length),
                integer_value(ValueKind::INTEGER, line as i64),
            ];
            return failure(standard_value(StandardConstructor::UTF8_INVALID, &fields));
        }
        // A copy first, as runtime.c made, so memory reports keep their
        // allocations.
        let copy = allocate(bytes.len() + 1, 1);
        ptr::copy_nonoverlapping(bytes.as_ptr(), copy, bytes.len());
        let mut arguments = [state, text_value(copy.cast::<c_char>(), bytes.len())];
        call.call(callback, &mut arguments)
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

const SHAPE: &CStr = c"verified host function has invalid runtime arguments";

/// `Stream.process_utf8_lines(path, state, fn(state, line))`: folds the
/// callback over the lines of `path` ("-" for standard input), each without
/// its newline or a carriage return before it. Each line's allocations are
/// reclaimed once the callback returns, keeping only the next state.
///
/// # Safety
/// Runs on a fiber that may allocate; the call and arguments are live.
#[inline(never)]
pub unsafe fn process_utf8_lines(call: ClockCall, arguments: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let runtime = call.runtime;
        let [path, initial, callback] = arguments else {
            return defect(SHAPE, i64::from(call.call_node));
        };
        let (path, callback) = (*path, *callback);
        if (*path).kind != ValueKind::TEXT || (*callback).kind != ValueKind::CALLABLE {
            return defect(SHAPE, i64::from(call.call_node));
        }
        if is_cancelled(runtime) {
            return cancelled();
        }
        let path_bytes =
            core::slice::from_raw_parts((*path).payload.text.bytes.cast::<u8>(), (*path).payload.text.length);
        // A C path stops at U+0000, so such a path names no file; the message
        // is the one the interpreter's file system reports.
        if path_bytes.contains(&0) {
            let fields = [path, {
                let message = c"file name contained an unexpected NUL byte";
                text_value(message.as_ptr(), message.count_bytes())
            }];
            return failure(standard_value(StandardConstructor::FILE_OPEN, &fields));
        }
        let owns_file = path_bytes != b"-";
        let mut file_bytes: Vec<u8> = Vec::new();
        let mut file_input = ByteInput { fd: -1, bytes: ptr::null_mut(), start: 0, end: 0, lock: ptr::null_mut() };
        let input: *mut ByteInput = if owns_file {
            // Text bytes are NUL-terminated, and hold no NUL here.
            file_input.fd = libc::open((*path).payload.text.bytes, libc::O_RDONLY | libc::O_CLOEXEC);
            if file_input.fd < 0 {
                return file_message(StandardConstructor::FILE_OPEN, path, libc::strerror(errno()));
            }
            // Freed below, so a loop over many files does not keep a buffer
            // for each.
            if file_bytes.try_reserve_exact(INPUT_BUFFER_BYTES).is_err() {
                libc::close(file_input.fd);
                return file_message(StandardConstructor::FILE_OPEN, path, libc::strerror(libc::ENOMEM));
            }
            file_input.bytes = file_bytes.as_mut_ptr();
            &raw mut file_input
        } else {
            STANDARD_INPUT.input.get()
        };
        let max = (*runtime).max_line_bytes as usize;
        let line = allocate(max + 1, 1);
        let mut length = 0usize;
        let mut line_number = 1u64;
        let mut state = *initial;
        let callable = (*callback).payload.callable;
        let mut scope = MaybeUninit::<AllocationScope>::uninit();
        let scope = scope.as_mut_ptr();
        young::scope_begin(current_carrier(), scope);
        let keep = |control: *mut Control| {
            let mut control = control;
            collector::collect(current_carrier(), scope, ptr::null_mut(), &raw mut control);
            control
        };
        // The input's lock is held while a line's bytes are taken (except
        // while `next` waits), and let go once the line ends (a newline,
        // the end, a failure or a line too long).
        let mut held = false;
        let mut control = loop {
            if !held {
                lock(input);
                held = true;
            }
            if is_cancelled(runtime) {
                unlock(input);
                break keep(cancelled());
            }
            let byte = next(runtime, input);
            if !matches!(byte, Next::Byte(b) if b != b'\n') || length == max {
                unlock(input);
                held = false;
            }
            let byte = match byte {
                Next::Cancelled => break keep(cancelled()),
                Next::Failed(error) => {
                    break keep(file_message(StandardConstructor::FILE_READ, path, libc::strerror(error)));
                }
                Next::End if length > 0 => {
                    let bytes = core::slice::from_raw_parts(line, length);
                    break keep(line_callback(call, callable, state, bytes, line_number, path));
                }
                Next::End => break keep(success(state)),
                Next::Byte(byte) => byte,
            };
            if byte == b'\n' {
                let content = if length > 0 && *line.add(length - 1) == b'\r' { length - 1 } else { length };
                let bytes = core::slice::from_raw_parts(line, content);
                let outcome = line_callback(call, callable, state, bytes, line_number, path);
                if (*outcome).kind != ControlKind::SUCCESS {
                    break keep(outcome);
                }
                state = (*outcome).value;
                collector::collect(current_carrier(), scope, &raw mut state, ptr::null_mut());
                length = 0;
                line_number += 1;
                continue;
            }
            if length == max {
                let fields = [
                    text_value((*path).payload.text.bytes, (*path).payload.text.length),
                    integer_value(ValueKind::INTEGER, i64::from((*runtime).max_line_bytes)),
                ];
                break keep(failure(standard_value(StandardConstructor::FILE_LINE_TOO_LONG, &fields)));
            }
            *line.add(length) = byte;
            length += 1;
        };
        drop(file_bytes);
        if owns_file
            && libc::close(file_input.fd) != 0
            && matches!((*control).kind, ControlKind::SUCCESS | ControlKind::FAILURE)
        {
            control = keep(file_message(StandardConstructor::FILE_CLOSE, path, libc::strerror(errno())));
        }
        end_measured_scope(scope, control)
    }
}

/// Parks the running task until `fd` is readable, or writable; false when
/// the task was cancelled meanwhile. Without the scheduler it polls, and a
/// stop signal interrupts the poll.
///
/// # Safety
/// `runtime` is live; runs on a fiber when the program starts the scheduler.
pub(crate) unsafe fn wait_for(runtime: *const Runtime, fd: c_int, writable: bool) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        if scheduler::requires_scheduler() {
            let mut request = SchedulerRequest::of(Scheduler::REQUEST_IO_WAIT);
            request.value = i64::from(fd);
            request.token = if writable { Scheduler::IO_WRITABLE } else { Scheduler::IO_READABLE };
            return !scheduler::request(&mut request);
        }
        let mut poll = libc::pollfd { fd, events: if writable { libc::POLLOUT } else { libc::POLLIN }, revents: 0 };
        libc::poll(&mut poll, 1, -1);
        !is_cancelled(runtime)
    }
}
