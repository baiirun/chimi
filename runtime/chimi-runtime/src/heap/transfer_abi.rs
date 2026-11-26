//! The C ABI of the capability walk (`transfer.rs`).

use core::ffi::c_void;

use super::layout::Value;
use super::transfer;

/// Walks the graph under `root` for the `CapabilityTransfer` `transfer`;
/// see [`transfer::captures`].
///
/// # Safety
/// As for [`transfer::captures`]; `probes` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_transfer_captures_v1(transfer: *mut c_void, root: *mut Value, probes: *mut u64) -> bool {
    // SAFETY: the caller's contract.
    unsafe { transfer::captures(transfer, root, &mut *probes) }
}
