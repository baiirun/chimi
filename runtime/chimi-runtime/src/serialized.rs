//! State that only the holder of one of `runtime.c`'s locks touches. This
//! `no_std` crate has no operating system lock of its own, and a spin lock
//! built on atomics would hide the calls' ordering from ThreadSanitizer, so
//! the C side holds a pthread mutex around every call that reaches it.

use core::cell::UnsafeCell;

pub(crate) struct Serialized<T>(UnsafeCell<T>);

// SAFETY: every exported function's caller holds the one lock that
// serializes access to the value.
unsafe impl<T: Send> Sync for Serialized<T> {}

impl<T> Serialized<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// # Safety
    /// The caller holds the `runtime.c` lock that guards this value, and
    /// `body` does not reenter it.
    pub(crate) unsafe fn with<R>(&self, body: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: the lock makes this the only reference.
        body(unsafe { &mut *self.0.get() })
    }
}
