//! The host cores of std/env and std/fs; `execution/host_operations.rs` is
//! the interpreter's, and the results match it. A std/fs result is a tuple
//! whose first field is a status: a non-negative result, or a portable
//! failure code that std/fs maps to FsError, the codes the interpreter
//! gives from Rust's `ErrorKind`. Its message is `strerror`'s text, which
//! Rust's `io::Error` prints too. Values are allocated in the order
//! `runtime.c` allocated them, so memory reports do not change.
//!
//! The open files are a [`FileTable`] of descriptors behind one lock: any
//! task, on any carrier, may hold a handle. A read or write borrows the
//! descriptor for the length of its system call; a close during one leaves
//! the descriptor open until it is given back, so the number is not reused
//! under the call. The table hands back each descriptor to close exactly
//! once, and it is closed after the lock is released.

use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;

use super::input::wait_for;
use super::standard::{defect, option_value, shape_defect, success};
use crate::files::{Closed, FileTable};
use crate::heap::allocator::current_carrier;
use crate::heap::layout::{AllocationKind, Control, ControlKind, Field, Intrinsic, Value, ValueKind};
use crate::heap::list;
use crate::heap::values::{allocate_kind, integer_value, new_control, new_value, text_value};
use crate::pthread_locked::PthreadLocked;

unsafe extern "C" {
    static environ: *const *const c_char;
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

const NOT_FOUND: i64 = -1;
const PERMISSION_DENIED: i64 = -2;
const ALREADY_EXISTS: i64 = -3;
const NOT_A_DIRECTORY: i64 = -4;
const IS_A_DIRECTORY: i64 = -5;
const NOT_EMPTY: i64 = -6;
const CLOSED: i64 = -7;
const OTHER: i64 = -8;

const READ_CHUNK_BYTES: usize = 64 * 1024;
const NUL_MESSAGE: &CStr = c"path contains a NUL byte";
const CLOSED_MESSAGE: &CStr = c"file is closed";

fn failure_code(error: c_int) -> i64 {
    match error {
        libc::ENOENT => NOT_FOUND,
        libc::EACCES | libc::EPERM => PERMISSION_DENIED,
        libc::EEXIST => ALREADY_EXISTS,
        libc::ENOTDIR => NOT_A_DIRECTORY,
        libc::EISDIR => IS_A_DIRECTORY,
        libc::ENOTEMPTY => NOT_EMPTY,
        _ => OTHER,
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

fn set_errno(value: c_int) {
    // SAFETY: the thread's errno location is always valid.
    unsafe { *errno_location() = value };
}

/// An Int.
///
/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn int(raw: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::INTEGER, raw) }
}

/// A Text of the NUL-terminated `text`.
///
/// # Safety
/// Runs on a fiber that may allocate; `text` is NUL-terminated.
#[inline(never)]
unsafe fn text(text: *const c_char) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { text_value(text, libc::strlen(text)) }
}

/// A Bytes holding a copy of `bytes`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn bytes(bytes: &[u8]) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = text_value(bytes.as_ptr().cast(), bytes.len());
        (*value).kind = ValueKind::BYTES;
        value
    }
}

/// An empty List.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn empty_list() -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { list::list_value(list::from_values(ptr::null(), 0), 0, 0) }
}

/// A tuple of `length` fields, which the caller fills; the allocator's fast
/// path inlines into every caller, so this one keeps it out of line.
///
/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn new_tuple(length: usize) -> (*mut Value, *mut Field) {
    // SAFETY: the caller's contract.
    unsafe {
        let tuple = new_value(ValueKind::TUPLE);
        (*tuple).payload.record.length = length as u32;
        let fields = allocate_kind::<Field>(length, AllocationKind::FIELDS);
        (*tuple).payload.record.fields = fields;
        (tuple, fields)
    }
}

/// A tuple of `values`.
///
/// # Safety
/// Runs on a fiber that may allocate; `values` are live.
#[inline(never)]
pub(super) unsafe fn tuple(values: &[*mut Value]) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let (tuple, fields) = new_tuple(values.len());
        for (index, &value) in values.iter().enumerate() {
            *fields.add(index) = Field { name: ptr::null(), value };
        }
        tuple
    }
}

/// `(status, message)`.
///
/// # Safety
/// Runs on a fiber that may allocate; `message` is NUL-terminated.
#[inline(never)]
unsafe fn status(status: i64, message: *const c_char) -> *mut Value {
    // SAFETY: the caller's contract; the Int is made before the Text, as in C.
    unsafe {
        let code = int(status);
        tuple(&[code, text(message)])
    }
}

/// The failure status of `error`.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn errno_status(error: c_int) -> *mut Value {
    // SAFETY: the caller's contract; strerror's text is NUL-terminated.
    unsafe { status(failure_code(error), libc::strerror(error)) }
}

/// The status of a system call that returned `result`, setting errno on
/// failure.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn result_status(result: c_int) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { if result == 0 { status(0, c"".as_ptr()) } else { errno_status(errno()) } }
}

/// Success with `values` as a tuple.
///
/// # Safety
/// Runs on a fiber that may allocate; `values` are live.
unsafe fn tuple_success(values: &[*mut Value]) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { success(tuple(values)) }
}

/// The running task was cancelled.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn cancelled() -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { new_control(ControlKind::CANCEL, ptr::null_mut(), ptr::null(), -1) }
}

/// A NUL-terminated copy of a Text, freed when dropped.
struct CPath(*mut c_char);

impl CPath {
    /// The copy of `path`'s text, or None when it holds U+0000: a C path
    /// stops there, so such a path names no file.
    ///
    /// # Safety
    /// `path` is a live Text.
    unsafe fn of(path: *const Value) -> Option<CPath> {
        // SAFETY: the caller's contract.
        unsafe {
            let (bytes, length) = ((*path).payload.text.bytes, (*path).payload.text.length);
            if !libc::memchr(bytes.cast(), 0, length).is_null() {
                return None;
            }
            let copy = libc::malloc(length + 1).cast::<c_char>();
            if copy.is_null() {
                chimi_runtime_exhausted_memory();
            }
            ptr::copy_nonoverlapping(bytes, copy, length);
            *copy.add(length) = 0;
            Some(CPath(copy))
        }
    }
}

impl Drop for CPath {
    fn drop(&mut self) {
        // SAFETY: the copy came from malloc and is freed once.
        unsafe { libc::free(self.0.cast()) };
    }
}

/// The open files, a handle being `slot | generation << 32`, so one used
/// after its file was closed fails with `CLOSED` instead of reaching
/// another file.
static FILES: PthreadLocked<FileTable<c_int>> = PthreadLocked::new(FileTable::new());

/// Opens descriptor `fd` and returns its handle.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_files_insert_v1(fd: c_int) -> i64 {
    FILES.with(|files| files.insert(fd))
}

/// Lends out the descriptor of open `handle`, to be given back with
/// [`chimi_files_give_v1`]; -1 when the handle names no open file.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_files_take_v1(handle: i64) -> c_int {
    FILES.with(|files| files.take(handle).copied().unwrap_or(-1))
}

/// Ends a loan [`chimi_files_take_v1`] began; the last user of a closed
/// file closes its descriptor.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_files_give_v1(handle: i64) {
    if let Some(fd) = FILES.with(|files| files.give(handle)) {
        // SAFETY: the table handed the descriptor back to close once.
        unsafe { libc::close(fd) };
    }
}

/// Closes `handle`; false when it was already closed. As the interpreter,
/// which drops the file, a close error is not reported.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_files_close_v1(handle: i64) -> bool {
    match FILES.with(|files| files.close(handle)) {
        Closed::NotOpen => false,
        Closed::Lent => true,
        Closed::Released(fd) => {
            // SAFETY: the table handed the descriptor back to close once.
            unsafe { libc::close(fd) };
            true
        }
    }
}

/// The intrinsic `intrinsic` of std/env or std/fs on `arguments`; a host
/// call checks `cancel` (the task's cancellation) before it enters the
/// operating system.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live; `runtime` is
/// the call's.
pub unsafe fn host_operation(
    runtime: *mut c_void,
    cancel: bool,
    intrinsic: u32,
    arguments: &[*mut Value],
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if intrinsic <= Intrinsic::ENV_HOST_CURRENT_DIRECTORY {
            if !arguments.is_empty() {
                return shape_defect(call_node);
            }
            if cancel {
                return cancelled();
            }
            return if intrinsic == Intrinsic::ENV_HOST_VARIABLES { variables() } else { current_directory(call_node) };
        }
        const ARITY: [usize; 13] = [1, 2, 1, 2, 1, 1, 1, 1, 1, 2, 1, 0, 1];
        if arguments.len() != ARITY[(intrinsic - Intrinsic::FS_HOST_OPEN) as usize] {
            return shape_defect(call_node);
        }
        if cancel {
            return cancelled();
        }
        let kind = |at: usize, kind: ValueKind| (*arguments[at]).kind == kind;
        match intrinsic {
            Intrinsic::FS_HOST_READ if kind(0, ValueKind::INTEGER) => {
                return read(runtime, (*arguments[0]).payload.integer);
            }
            Intrinsic::FS_HOST_WRITE if kind(0, ValueKind::INTEGER) && kind(1, ValueKind::BYTES) => {
                return write(runtime, (*arguments[0]).payload.integer, arguments[1]);
            }
            Intrinsic::FS_HOST_CLOSE if kind(0, ValueKind::INTEGER) => {
                if !chimi_files_close_v1((*arguments[0]).payload.integer) {
                    return success(status(CLOSED, CLOSED_MESSAGE.as_ptr()));
                }
                return success(status(0, c"".as_ptr()));
            }
            Intrinsic::FS_HOST_TEMPORARY_DIRECTORY => return temporary_directory(call_node),
            Intrinsic::FS_TEST_ROOT if kind(0, ValueKind::TEXT) => {
                let root = (*arguments[0]).payload.text;
                if root.length == 0 || *root.bytes != b'/' as c_char {
                    return defect(c"Fs.files_within needs an absolute root", i64::from(call_node));
                }
                return success(arguments[0]);
            }
            Intrinsic::FS_HOST_READ
            | Intrinsic::FS_HOST_WRITE
            | Intrinsic::FS_HOST_CLOSE
            | Intrinsic::FS_TEST_ROOT => return shape_defect(call_node),
            _ => {}
        }
        if !kind(0, ValueKind::TEXT) || (intrinsic == Intrinsic::FS_HOST_RENAME && !kind(1, ValueKind::TEXT)) {
            return shape_defect(call_node);
        }
        let path = CPath::of(arguments[0]);
        let second = if intrinsic == Intrinsic::FS_HOST_RENAME { CPath::of(arguments[1]) } else { None };
        let (Some(path), true) = (path, intrinsic != Intrinsic::FS_HOST_RENAME || second.is_some()) else {
            return nul_path(intrinsic);
        };
        let path = path.0;
        match intrinsic {
            Intrinsic::FS_HOST_OPEN => {
                let fd = libc::open(path, libc::O_RDONLY | libc::O_CLOEXEC);
                let mut stat: libc::stat = core::mem::zeroed();
                if fd < 0 {
                    return success(errno_status(errno()));
                }
                let is_directory = |stat: &libc::stat| stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
                if libc::fstat(fd, &mut stat) != 0 || is_directory(&stat) || register(fd) != 0 {
                    let error = if errno() != 0 && !is_directory(&stat) { errno() } else { libc::EISDIR };
                    libc::close(fd);
                    return success(errno_status(error));
                }
                success(status(chimi_files_insert_v1(fd), c"".as_ptr()))
            }
            Intrinsic::FS_HOST_CREATE => {
                if !kind(1, ValueKind::INTEGER) {
                    return shape_defect(call_node);
                }
                let mode = (*arguments[1]).payload.integer;
                let flags = libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_CLOEXEC
                    | match mode {
                        0 => libc::O_TRUNC,
                        1 => libc::O_APPEND,
                        _ => libc::O_EXCL,
                    };
                let fd = libc::open(path, flags, 0o666 as libc::c_uint);
                if fd < 0 {
                    return success(errno_status(errno()));
                }
                if register(fd) != 0 {
                    let error = errno();
                    libc::close(fd);
                    return success(errno_status(error));
                }
                success(status(chimi_files_insert_v1(fd), c"".as_ptr()))
            }
            Intrinsic::FS_HOST_METADATA => metadata(path),
            Intrinsic::FS_HOST_LIST => list_directory(path),
            Intrinsic::FS_HOST_CREATE_DIRECTORY => success(result_status(libc::mkdir(path, 0o777))),
            Intrinsic::FS_HOST_REMOVE => {
                let mut stat: libc::stat = core::mem::zeroed();
                if libc::lstat(path, &mut stat) != 0 {
                    return success(errno_status(errno()));
                }
                let removed =
                    if stat.st_mode & libc::S_IFMT == libc::S_IFDIR { libc::rmdir(path) } else { libc::unlink(path) };
                success(result_status(removed))
            }
            Intrinsic::FS_HOST_RENAME => {
                let second = second.as_ref().map_or(ptr::null_mut(), |second| second.0);
                success(result_status(libc::rename(path, second)))
            }
            Intrinsic::FS_HOST_IS_DIRECTORY => {
                let mut stat: libc::stat = core::mem::zeroed();
                if libc::lstat(path, &mut stat) != 0 {
                    return success(errno_status(errno()));
                }
                success(status(i64::from(stat.st_mode & libc::S_IFMT == libc::S_IFDIR), c"".as_ptr()))
            }
            _ => shape_defect(call_node),
        }
    }
}

/// What a call on a path holding U+0000 answers: the failure tuple of its
/// result's shape.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn nul_path(intrinsic: u32) -> *mut Control {
    // SAFETY: the caller's contract; each tuple's fields are made in order.
    unsafe {
        match intrinsic {
            Intrinsic::FS_HOST_METADATA => {
                let code = int(OTHER);
                let (kind, size, modified) = (int(0), int(0), int(0));
                tuple_success(&[code, kind, size, modified, text(NUL_MESSAGE.as_ptr())])
            }
            Intrinsic::FS_HOST_LIST => {
                let code = int(OTHER);
                let names = empty_list();
                tuple_success(&[code, names, text(NUL_MESSAGE.as_ptr())])
            }
            _ => success(status(OTHER, NUL_MESSAGE.as_ptr())),
        }
    }
}

/// A descriptor that is not a regular file (a pipe, a terminal, a socket) is
/// made non-blocking, so a read or write that would block parks the task
/// instead of stopping every task on its carrier.
///
/// # Safety
/// `fd` is open.
unsafe fn register(fd: c_int) -> c_int {
    // SAFETY: the caller's contract.
    unsafe {
        let mut stat: libc::stat = core::mem::zeroed();
        if libc::fstat(fd, &mut stat) != 0 {
            return -1;
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags != -1 {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
        0
    }
}

/// std/env's variables in `environ`'s order; a pair whose name or value is
/// not UTF-8 is left out.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn variables() -> *mut Control {
    // SAFETY: the caller's contract; `environ` is a NULL-terminated array of
    // NUL-terminated entries.
    unsafe {
        let mut list = empty_list();
        let mut entry = environ;
        while !entry.is_null() && !(*entry).is_null() {
            let bytes = CStr::from_ptr(*entry).to_bytes();
            entry = entry.add(1);
            // As Rust's vars_os: a name may start with '=', so the separator
            // is the first '=' after the first byte.
            let Some(equals) = bytes.iter().skip(1).position(|&byte| byte == b'=').map(|at| at + 1) else {
                continue;
            };
            let (name, value) = (&bytes[..equals], &bytes[equals + 1..]);
            if core::str::from_utf8(name).is_err() || core::str::from_utf8(value).is_err() {
                continue;
            }
            let (pair, fields) = new_tuple(2);
            *fields = Field { name: ptr::null(), value: text_value(name.as_ptr().cast(), name.len()) };
            *fields.add(1) = Field { name: ptr::null(), value: text_value(value.as_ptr().cast(), value.len()) };
            list = list::list_push(list, pair);
        }
        success(list)
    }
}

/// The working directory, or None when it is not UTF-8 or cannot be read.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn current_directory(call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; each buffer holds `capacity` bytes.
    unsafe {
        let mut capacity = 256;
        loop {
            let buffer = libc::malloc(capacity).cast::<c_char>();
            if buffer.is_null() {
                return defect(c"out of memory reading the working directory", i64::from(call_node));
            }
            if !libc::getcwd(buffer, capacity).is_null() {
                let found = CStr::from_ptr(buffer);
                let path = if found.to_str().is_ok() { text(buffer) } else { ptr::null_mut() };
                libc::free(buffer.cast());
                return option_value(path);
            }
            libc::free(buffer.cast());
            if errno() != libc::ERANGE {
                return option_value(ptr::null_mut());
            }
            capacity *= 2;
        }
    }
}

/// `(status, names, message)`: the names in the directory at `path`, sorted
/// by their bytes, without `.` and `..`.
///
/// # Safety
/// Runs on a fiber that may allocate; `path` is NUL-terminated.
unsafe fn list_directory(path: *const c_char) -> *mut Control {
    // SAFETY: the caller's contract; every name is strdup's, freed once.
    unsafe {
        let directory = libc::opendir(path);
        if directory.is_null() {
            let error = errno();
            let code = int(failure_code(error));
            let names = empty_list();
            return tuple_success(&[code, names, text(libc::strerror(error))]);
        }
        let mut names = Names(alloc::vec::Vec::new());
        let mut failure: Option<&CStr> = None;
        let mut error = 0;
        loop {
            set_errno(0);
            let entry = libc::readdir(directory);
            if entry.is_null() {
                error = errno();
                break;
            }
            let name = CStr::from_ptr((*entry).d_name.as_ptr());
            if name == c"." || name == c".." {
                continue;
            }
            if name.to_str().is_err() {
                failure = Some(c"a name is not UTF-8");
                break;
            }
            if names.0.try_reserve(1).is_err() {
                chimi_runtime_exhausted_memory();
            }
            let copy = libc::strdup(name.as_ptr());
            if copy.is_null() {
                chimi_runtime_exhausted_memory();
            }
            names.0.push(copy);
        }
        libc::closedir(directory);
        let mut list = empty_list();
        if failure.is_none() && error == 0 {
            // A slice sort would add its code to every program; qsort is in
            // the C library already.
            if !names.0.is_empty() {
                libc::qsort(names.0.as_mut_ptr().cast(), names.0.len(), size_of::<*mut c_char>(), Some(compare_names));
            }
            for &name in &names.0 {
                list = list::list_push(list, text(name));
            }
        }
        drop(names);
        let code = int(match (failure, error) {
            (Some(_), _) => OTHER,
            (None, 0) => 0,
            (None, error) => failure_code(error),
        });
        let names = if failure.is_some() || error != 0 { empty_list() } else { list };
        let message = match (failure, error) {
            (Some(failure), _) => failure.as_ptr(),
            (None, 0) => c"".as_ptr(),
            (None, error) => libc::strerror(error),
        };
        tuple_success(&[code, names, text(message)])
    }
}

/// strdup'd names, freed when dropped.
struct Names(alloc::vec::Vec<*mut c_char>);

impl Drop for Names {
    fn drop(&mut self) {
        for &name in &self.0 {
            // SAFETY: each name came from strdup and is freed once.
            unsafe { libc::free(name.cast()) };
        }
    }
}

unsafe extern "C" fn compare_names(left: *const c_void, right: *const c_void) -> c_int {
    // SAFETY: qsort passes pointers to two of the names.
    unsafe { libc::strcmp(*left.cast::<*const c_char>(), *right.cast::<*const c_char>()) }
}

/// `(status, kind, size, modified, message)` of the file at `path`: kind 0
/// for a file, 1 for a directory, 2 otherwise, and the modification time in
/// Unix nanoseconds.
///
/// # Safety
/// Runs on a fiber that may allocate; `path` is NUL-terminated.
unsafe fn metadata(path: *const c_char) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let mut stat: libc::stat = core::mem::zeroed();
        if libc::stat(path, &mut stat) != 0 {
            let error = errno();
            let code = int(failure_code(error));
            let (kind, size, modified) = (int(0), int(0), int(0));
            return tuple_success(&[code, kind, size, modified, text(libc::strerror(error))]);
        }
        let kind = match stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => 0,
            libc::S_IFDIR => 1,
            _ => 2,
        };
        let modified = stat.st_mtime as i64 * 1_000_000_000 + stat.st_mtime_nsec as i64;
        let code = int(0);
        let (kind, size, modified) = (int(kind), int(stat.st_size as i64), int(modified));
        tuple_success(&[code, kind, size, modified, text(c"".as_ptr())])
    }
}

/// The running carrier's read buffer, allocated on first use: it is too
/// large for a fiber stack, and most programs never read a file.
///
/// # Safety
/// Runs on a carrier.
unsafe fn read_chunk() -> *mut c_char {
    // SAFETY: the caller's contract; the buffer is the carrier's own.
    unsafe {
        let here = current_carrier();
        if (*here).fs_read_chunk.is_null() {
            (*here).fs_read_chunk = libc::malloc(READ_CHUNK_BYTES).cast();
            if (*here).fs_read_chunk.is_null() {
                chimi_runtime_exhausted_memory();
            }
        }
        (*here).fs_read_chunk
    }
}

/// `(status, bytes, message)`: up to 64 KiB from the file `handle`, status 1
/// with some bytes and 0 at its end, waiting while a pipe or terminal has
/// none.
///
/// # Safety
/// Runs on a fiber that may allocate; `runtime` is the call's.
unsafe fn read(runtime: *mut c_void, handle: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        loop {
            // Take the descriptor and the buffer again on every pass: the
            // wait below parks, another task may close the handle meanwhile,
            // and the fiber may resume on another carrier.
            let fd = chimi_files_take_v1(handle);
            if fd < 0 {
                let code = int(CLOSED);
                let empty = bytes(&[]);
                return tuple_success(&[code, empty, text(CLOSED_MESSAGE.as_ptr())]);
            }
            let chunk = read_chunk();
            let count = libc::read(fd, chunk.cast(), READ_CHUNK_BYTES);
            let error = errno();
            chimi_files_give_v1(handle);
            if count >= 0 {
                let code = int(i64::from(count > 0));
                let read = bytes(core::slice::from_raw_parts(chunk.cast::<u8>(), count as usize));
                return tuple_success(&[code, read, text(c"".as_ptr())]);
            }
            if error == libc::EINTR {
                continue;
            }
            if error == libc::EAGAIN || error == libc::EWOULDBLOCK {
                if !wait_for(runtime.cast(), fd, false) {
                    return cancelled();
                }
                continue;
            }
            let code = int(failure_code(error));
            let empty = bytes(&[]);
            return tuple_success(&[code, empty, text(libc::strerror(error))]);
        }
    }
}

/// `(status, message)` of writing all of `data` to the file `handle`,
/// waiting while a pipe or terminal is full.
///
/// # Safety
/// Runs on a fiber that may allocate; `runtime` is the call's; `data` is a
/// live Bytes.
unsafe fn write(runtime: *mut c_void, handle: i64, data: *const Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let (start, length) = ((*data).payload.text.bytes, (*data).payload.text.length);
        let mut written = 0;
        loop {
            let fd = chimi_files_take_v1(handle);
            if fd < 0 {
                return success(status(CLOSED, CLOSED_MESSAGE.as_ptr()));
            }
            if written == length {
                chimi_files_give_v1(handle);
                break;
            }
            let count = libc::write(fd, start.add(written).cast(), length - written);
            let error = errno();
            chimi_files_give_v1(handle);
            if count > 0 {
                written += count as usize;
                if written < length {
                    continue;
                }
                break;
            }
            if count < 0 && error == libc::EINTR {
                continue;
            }
            if count < 0 && (error == libc::EAGAIN || error == libc::EWOULDBLOCK) {
                if !wait_for(runtime.cast(), fd, true) {
                    return cancelled();
                }
                continue;
            }
            return success(errno_status(if count == 0 { libc::EIO } else { error }));
        }
        success(status(0, c"".as_ptr()))
    }
}

/// A new directory under $TMPDIR (else /tmp), as Rust's temp_dir finds it,
/// with its symbolic links resolved.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn temporary_directory(call_node: u32) -> *mut Control {
    const SUFFIX: &[u8] = b"/chimi-test-XXXXXX\0";
    // SAFETY: the caller's contract; the template holds the base, the
    // suffix and its NUL.
    unsafe {
        let mut base = libc::getenv(c"TMPDIR".as_ptr()).cast_const();
        if base.is_null() || *base == 0 {
            base = c"/tmp".as_ptr();
        }
        let mut length = libc::strlen(base);
        while length > 1 && *base.add(length - 1) == b'/' as c_char {
            length -= 1;
        }
        let template = libc::malloc(length + SUFFIX.len()).cast::<c_char>();
        if template.is_null() {
            chimi_runtime_exhausted_memory();
        }
        ptr::copy_nonoverlapping(base, template, length);
        ptr::copy_nonoverlapping(SUFFIX.as_ptr().cast(), template.add(length), SUFFIX.len());
        let resolved =
            if libc::mkdtemp(template).is_null() { ptr::null_mut() } else { libc::realpath(template, ptr::null_mut()) };
        libc::free(template.cast());
        if resolved.is_null() {
            return defect(c"cannot create a temporary directory", i64::from(call_node));
        }
        let path = if CStr::from_ptr(resolved).to_str().is_ok() { text(resolved) } else { ptr::null_mut() };
        libc::free(resolved.cast());
        if path.is_null() {
            return defect(c"cannot create a temporary directory: it is not UTF-8", i64::from(call_node));
        }
        success(path)
    }
}
