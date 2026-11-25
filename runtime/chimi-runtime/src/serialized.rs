//! State that only the holder of one of `runtime.c`'s locks touches. This
//! `no_std` crate has no operating system lock of its own, and a spin lock
//! built on atomics would hide the calls' ordering from ThreadSanitizer, so
//! the C side holds a pthread mutex around every call that reaches it.
//!
//! ThreadSanitizer cannot see inside this crate, so a call made without the
//! lock would race unseen. Each value therefore keeps a busy flag that a call
//! sets on entry: finding it already set means two calls overlap, which only
//! a missing lock allows, and the program aborts rather than share the value.
//! Under the lock the flag is never contended, so it costs one exchange and
//! one store per call.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

unsafe extern "C" {
    fn abort() -> !;
}

pub(crate) struct Serialized<T> {
    value: UnsafeCell<T>,
    busy: AtomicBool,
}

// SAFETY: every exported function's caller holds the one lock that
// serializes access to the value, and `with` aborts on an overlapping call.
unsafe impl<T: Send> Sync for Serialized<T> {}

impl<T> Serialized<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self { value: UnsafeCell::new(value), busy: AtomicBool::new(false) }
    }

    /// # Safety
    /// The caller holds the `runtime.c` lock that guards this value, and
    /// `body` does not reenter it.
    pub(crate) unsafe fn with<R>(&self, body: impl FnOnce(&mut T) -> R) -> R {
        if self.busy.swap(true, Ordering::Acquire) {
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
