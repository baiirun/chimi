//! State that only the holder of one of `runtime.c`'s locks touches. A spin
//! lock built on atomics would hide the calls' ordering from ThreadSanitizer,
//! so the C side holds a pthread mutex around every call that reaches it.
//! State that Rust code reaches on its own takes a pthread mutex here instead
//! ([`crate::pthread_locked`]).
//!
//! ThreadSanitizer cannot see inside this crate, so a call made without the
//! lock would race unseen. Each value therefore keeps a busy flag that a call
//! sets on entry: finding it already set means two calls overlap, which only
//! a missing lock allows, and the program aborts rather than share the value.
//! Under the lock the flag is never contended, so it costs one exchange and
//! one store per call.
//!
//! One overlap is allowed: without the scheduler, the allocator ends the
//! program from inside a call when memory runs out, and the exit handlers
//! may then reach the value. The allocator marks the
//! program as ending first ([`note_exiting`]), and the failed allocation has
//! not changed the value, so the drain may read it.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

unsafe extern "C" {
    fn abort() -> !;
}

/// Set once the program is ending from inside a call.
static EXITING: AtomicBool = AtomicBool::new(false);

/// Lets the exit handlers reach a value whose call the program is ending
/// from. Only the allocator's out-of-memory exit calls it.
pub fn note_exiting() {
    EXITING.store(true, Ordering::Relaxed);
}

pub(crate) struct Serialized<T> {
    value: UnsafeCell<T>,
    busy: AtomicBool,
}

// SAFETY: every exported function's caller holds the one lock that
// serializes access to the value, and `with` aborts on an overlapping call
// except while the program ends from inside one.
unsafe impl<T: Send> Sync for Serialized<T> {}

impl<T> Serialized<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self { value: UnsafeCell::new(value), busy: AtomicBool::new(false) }
    }

    /// # Safety
    /// The caller holds the `runtime.c` lock that guards this value, and
    /// `body` does not reenter it.
    pub(crate) unsafe fn with<R>(&self, body: impl FnOnce(&mut T) -> R) -> R {
        if self.busy.swap(true, Ordering::Acquire) && !EXITING.load(Ordering::Relaxed) {
            // SAFETY: abort takes no arguments and never returns.
            unsafe { abort() }
        }
        // SAFETY: the lock, checked by `busy`, makes this the only reference.
        let result = body(unsafe { &mut *self.value.get() });
        self.busy.store(false, Ordering::Release);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_one_after_another_share_the_value() {
        let shared = Serialized::new(0u32);
        // SAFETY: one thread, so the calls never overlap.
        unsafe {
            shared.with(|value| *value += 1);
            assert_eq!(shared.with(|value| *value), 1);
        }
    }
}
