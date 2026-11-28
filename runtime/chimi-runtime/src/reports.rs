//! The report files a run writes at exit (CHI-305 step 5, phase 8): the
//! operation profile (`--profile-file`).

use alloc::vec::Vec;
use core::ffi::{CStr, c_char};

use crate::heap::allocator::profile_rows;
use crate::heap::layout::{Node, Origin};
use crate::telemetry::Out;

unsafe extern "C" {
    static CHIMI_NODES: [Node; 0];
    static CHIMI_ORIGINS: [Origin; 0];
    static CHIMI_NODE_COUNT: u32;
    fn chimi_runtime_exhausted_memory() -> !;
}

/// A report file opened with `mode`, closed in every child process the run
/// starts (std/exec), so a child cannot hold it open or write into it.
///
/// # Safety
/// `path` and `mode` are C strings.
unsafe fn open_report(path: *const c_char, mode: &CStr) -> *mut libc::FILE {
    // SAFETY: the caller's contract.
    unsafe {
        let output = libc::fopen(path, mode.as_ptr());
        if !output.is_null() && libc::fcntl(libc::fileno(output), libc::F_SETFD, libc::FD_CLOEXEC) != 0 {
            let error = *errno();
            libc::fclose(output);
            *errno() = error;
            return core::ptr::null_mut();
        }
        output
    }
}

#[cfg(target_os = "linux")]
fn errno() -> *mut i32 {
    // SAFETY: the calling thread's errno.
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
fn errno() -> *mut i32 {
    // SAFETY: the calling thread's errno.
    unsafe { libc::__error() }
}

/// Flushes and closes `output`; whether either failed.
///
/// # Safety
/// `output` is an open stream.
unsafe fn close_output(output: *mut libc::FILE) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        let failed = libc::fflush(output) != 0;
        (libc::fclose(output) != 0) || failed
    }
}

/// `cannot open <what> file <path>: <reason>` on stderr, for the `errno` of
/// the failed open.
///
/// # Safety
/// `path` is a C string.
unsafe fn report_open_failure(what: &[u8], path: *const c_char) {
    // SAFETY: the caller's contract; strerror returns a C string.
    unsafe {
        let reason = CStr::from_ptr(libc::strerror(*errno())).to_bytes();
        let mut line = Vec::new();
        for part in [b"cannot open ".as_slice(), what, b" file ", CStr::from_ptr(path).to_bytes(), b": ", reason, b"\n"] {
            line.extend_from_slice(part);
        }
        libc::write(2, line.as_ptr().cast(), line.len());
    }
}

/// Writes the profile to `path`, a new file: one JSON object with every
/// evaluated node, most allocated bytes first, then most evaluations, then
/// node id, so the same program always writes the same profile. Returns
/// whether writing failed; without a path there is nothing to write.
///
/// # Safety
/// `path` is null or a C string; no fiber runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_write_profile_v1(path: *const c_char) -> bool {
    if path.is_null() {
        return false;
    }
    // SAFETY: the caller's contract; the profile has a row per node.
    unsafe {
        let output = open_report(path, c"wx");
        if output.is_null() {
            report_open_failure(b"profile", path);
            return true;
        }
        let rows = profile_rows();
        let count = CHIMI_NODE_COUNT as usize;
        let mut order: Vec<u32> = Vec::new();
        if order.try_reserve_exact(count).is_err() {
            chimi_runtime_exhausted_memory();
        }
        order.extend((0..CHIMI_NODE_COUNT).filter(|&node| (*rows.add(node as usize)).evaluations != 0));
        order.sort_unstable_by(|&a, &b| {
            let (x, y) = (&*rows.add(a as usize), &*rows.add(b as usize));
            y.allocated_bytes.cmp(&x.allocated_bytes).then(y.evaluations.cmp(&x.evaluations)).then(a.cmp(&b))
        });
        let out = Out(output);
        out.bytes(b"{\"schema\":\"chimi.runtime-profile.v1\",\"operations\":[");
        for (index, &node) in order.iter().enumerate() {
            let row = &*rows.add(node as usize);
            let shape = &*(&raw const CHIMI_NODES).cast::<Node>().add(node as usize);
            let origin = &*(&raw const CHIMI_ORIGINS).cast::<Origin>().add(shape.origin as usize);
            out.bytes(if index > 0 { b",{\"core_node_id\":" } else { b"{\"core_node_id\":" });
            out.unsigned(u64::from(node));
            for (key, value) in [
                (&b",\"origin_id\":"[..], shape.origin),
                (b",\"file_id\":", origin.file_id),
                (b",\"source_start\":", origin.start),
                (b",\"source_end\":", origin.end),
            ] {
                out.bytes(key);
                out.unsigned(u64::from(value));
            }
            out.bytes(b",\"operation\":");
            out.json_c(row.operation);
            for (key, value) in [
                (&b",\"evaluations\":"[..], row.evaluations),
                (b",\"allocations\":", row.allocations),
                (b",\"allocated_bytes\":", row.allocated_bytes),
            ] {
                out.bytes(key);
                out.unsigned(value);
            }
            out.bytes(b"}");
        }
        out.bytes(b"]}\n");
        close_output(output)
    }
}
