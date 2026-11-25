//! The C view of [`crate::handles`]: the native runtime's binding handle
//! slots. A handle value may reach tasks on any carrier, so `runtime.c`
//! makes every call under `binding_handle_lock`, as it does for the open-file
//! table (see `files_abi`). That lock is taken after the scheduler's kernel
//! lock and is never held across a scheduler request or a release; no call
//! here makes one. A kind is a `const ChimiBindingType *`, a state the
//! binding's `void *`, and a runner the running `ChimiBindingCall *`, all
//! opaque here.

use core::ffi::c_void;

use crate::handles::{Closed, Handles, Release};
use crate::serialized::Serialized;

/// An opaque pointer the table only stores and compares.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Opaque(*const c_void);

// SAFETY: the table never dereferences it; it only hands it back, under the
// lock that serializes every call.
unsafe impl Send for Opaque {}

static HANDLES: Serialized<Handles<Opaque, Opaque, Opaque>> = Serialized::new(Handles::new());

/// A state handed back for the caller to release once the lock is let go;
/// `kind` is null when there is none.
#[repr(C)]
pub struct ChimiHandleRelease {
    kind: *const c_void,
    state: *mut c_void,
}

fn give(release: Option<Release<Opaque, Opaque>>, out: *mut ChimiHandleRelease) -> bool {
    let Some(release) = release else { return false };
    if !out.is_null() {
        // SAFETY: the caller passes a writable record.
        unsafe {
            out.write(ChimiHandleRelease { kind: release.kind.0, state: release.state.0.cast_mut() });
        }
    }
    true
}

/// Opens a handle of `kind` holding `state`, writing its slot and generation;
/// false when the table is full (the state is still the caller's to
/// release). Running out of memory to grow it ends the program.
/// # Safety
/// The caller holds `binding_handle_lock`; `slot` and `generation` are
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_open_v1(
    kind: *const c_void,
    state: *mut c_void,
    slot: *mut u32,
    generation: *mut u32,
) -> bool {
    // SAFETY: the caller holds the lock.
    let opened = unsafe { HANDLES.with(|handles| handles.open(Opaque(kind), Opaque(state))) };
    let Some((opened_slot, opened_generation)) = opened else { return false };
    // SAFETY: the caller passes writable outputs.
    unsafe {
        slot.write(opened_slot);
        generation.write(opened_generation);
    }
    true
}

/// The state of the open handle (slot, generation) when its kind is `kind`,
/// or of any kind when `kind` is null; null when it is not open.
/// # Safety
/// The caller holds `binding_handle_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_state_v1(slot: u32, generation: u32, kind: *const c_void) -> *mut c_void {
    // SAFETY: the caller holds the lock.
    match unsafe { HANDLES.with(|handles| handles.get(slot, generation)) } {
        Some((open, state)) if kind.is_null() || open == Opaque(kind) => state.0.cast_mut(),
        _ => core::ptr::null_mut(),
    }
}

/// True when (slot, generation) is open.
/// # Safety
/// The caller holds `binding_handle_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_is_open_v1(slot: u32, generation: u32) -> bool {
    // SAFETY: the caller holds the lock.
    unsafe { HANDLES.with(|handles| handles.get(slot, generation).is_some()) }
}

/// The handle was not open (of the asked kind).
pub const CLOSE_NOT_OPEN: i32 = 0;
/// Closed; a holder releases the state when it lets go.
pub const CLOSE_HELD: i32 = 1;
/// Closed; `out` holds the state to release.
pub const CLOSE_RELEASED: i32 = 2;

/// Closes the open handle (slot, generation) of `kind`, or of any kind when
/// `kind` is null.
/// # Safety
/// The caller holds `binding_handle_lock`; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_close_v1(
    slot: u32,
    generation: u32,
    kind: *const c_void,
    out: *mut ChimiHandleRelease,
) -> i32 {
    let kind = (!kind.is_null()).then_some(Opaque(kind));
    // SAFETY: the caller holds the lock.
    match unsafe { HANDLES.with(|handles| handles.close(slot, generation, kind)) } {
        Closed::NotOpen => CLOSE_NOT_OPEN,
        Closed::Held => CLOSE_HELD,
        Closed::Released(release) => {
            give(Some(release), out);
            CLOSE_RELEASED
        }
    }
}

/// Counts one more call holding the open handle (slot, generation); false
/// when it is not open.
/// # Safety
/// The caller holds `binding_handle_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_hold_v1(slot: u32, generation: u32) -> bool {
    // SAFETY: the caller holds the lock.
    unsafe { HANDLES.with(|handles| handles.hold(slot, generation)) }
}

/// Counts one call holding `slot` fewer; true when `out` now holds the state
/// of a handle closed meanwhile, to release.
/// # Safety
/// The caller holds `binding_handle_lock`; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_let_go_v1(slot: u32, out: *mut ChimiHandleRelease) -> bool {
    // SAFETY: the caller holds the lock.
    give(unsafe { HANDLES.with(|handles| handles.let_go(slot)) }, out)
}

/// The call running on `slot`, or null.
/// # Safety
/// The caller holds `binding_handle_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_runner_v1(slot: u32) -> *const c_void {
    // SAFETY: the caller holds the lock.
    unsafe { HANDLES.with(|handles| handles.runner(slot)) }.map_or(core::ptr::null(), |runner| runner.0)
}

/// Starts `runner` running on `slot`, or stops the one running when it is
/// null.
/// # Safety
/// The caller holds `binding_handle_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_set_runner_v1(slot: u32, runner: *const c_void) {
    let runner = (!runner.is_null()).then_some(Opaque(runner));
    // SAFETY: the caller holds the lock.
    unsafe { HANDLES.with(|handles| handles.set_runner(slot, runner)) }
}

/// Slots made so far, slot 0 included.
/// # Safety
/// The caller holds `binding_handle_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_count_v1() -> u32 {
    // SAFETY: the caller holds the lock.
    unsafe { HANDLES.with(|handles| handles.len()) }
}

/// At the program's end: true when `out` holds the state of `slot`, open or
/// closed while held, to release.
/// # Safety
/// The caller holds `binding_handle_lock`; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_handles_end_v1(slot: u32, out: *mut ChimiHandleRelease) -> bool {
    // SAFETY: the caller holds the lock.
    give(unsafe { HANDLES.with(|handles| handles.end(slot)) }, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The one test of the shared table, so no other test races it.
    #[test]
    fn a_handle_gives_its_state_only_for_its_own_kind() {
        let (kind, other) = (1u8, 2u8);
        let state = 3u8;
        let (kind, other) = (&raw const kind as *const c_void, &raw const other as *const c_void);
        let state = (&raw const state).cast_mut().cast::<c_void>();
        let (mut slot, mut generation) = (0, 0);
        // SAFETY: the test is the only caller, so it stands in for the lock.
        unsafe {
            assert!(chimi_handles_open_v1(kind, state, &mut slot, &mut generation));
            assert_eq!(chimi_handles_state_v1(slot, generation, kind), state);
            assert_eq!(chimi_handles_state_v1(slot, generation, core::ptr::null()), state);
            assert!(chimi_handles_state_v1(slot, generation, other).is_null());
            let mut release = ChimiHandleRelease { kind: core::ptr::null(), state: core::ptr::null_mut() };
            assert_eq!(chimi_handles_close_v1(slot, generation, other, &mut release), CLOSE_NOT_OPEN);
            assert_eq!(chimi_handles_close_v1(slot, generation, kind, &mut release), CLOSE_RELEASED);
            assert_eq!((release.kind, release.state), (kind, state));
            assert!(chimi_handles_state_v1(slot, generation, kind).is_null());
        }
    }
}
