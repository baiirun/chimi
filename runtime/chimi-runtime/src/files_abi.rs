//! The C view of [`crate::files`]: the native runtime's open-file table,
//! holding descriptors. Any task, on any carrier thread, may hold a handle,
//! so `runtime.c` makes every call under its own lock (`fs_file_lock`), as
//! it does for the `random` stream: this `no_std` crate has no operating
//! system lock of its own, and a spin lock built on atomics would hide the
//! calls' ordering from ThreadSanitizer, which then reports the descriptor
//! closes these calls hand back as races. No call makes a system call;
//! `runtime.c` closes a descriptor they hand back after releasing the lock.

use crate::files::{Closed, FileTable};
use crate::serialized::Serialized;

static FILES: Serialized<FileTable<i32>> = Serialized::new(FileTable::new());

/// Opens descriptor `fd` and returns its handle.
/// # Safety
/// The caller holds `fs_file_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_files_insert_v1(fd: i32) -> i64 {
    // SAFETY: the caller holds the lock.
    unsafe { FILES.with(|files| files.insert(fd)) }
}

/// Lends out the descriptor of open `handle`, to be given back with
/// `chimi_files_give_v1`; -1 when the handle names no open file.
/// # Safety
/// The caller holds `fs_file_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_files_take_v1(handle: i64) -> i32 {
    // SAFETY: the caller holds the lock.
    unsafe { FILES.with(|files| files.take(handle).copied().unwrap_or(-1)) }
}

/// Ends a loan `chimi_files_take_v1` began. Returns the descriptor for the
/// caller to close when the file was closed during the loan and this was
/// its last loan, else -1.
/// # Safety
/// The caller holds `fs_file_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_files_give_v1(handle: i64) -> i32 {
    // SAFETY: the caller holds the lock.
    unsafe { FILES.with(|files| files.give(handle).unwrap_or(-1)) }
}

/// The handle named no open file.
pub const FILES_NOT_OPEN: i32 = -2;

/// Closes `handle`. Returns the descriptor for the caller to close, -1 when
/// a loan still holds it (its last give returns it), or `FILES_NOT_OPEN`.
/// # Safety
/// The caller holds `fs_file_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_files_close_v1(handle: i64) -> i32 {
    // SAFETY: the caller holds the lock.
    match unsafe { FILES.with(|files| files.close(handle)) } {
        Closed::NotOpen => FILES_NOT_OPEN,
        Closed::Lent => -1,
        Closed::Released(fd) => fd,
    }
}
