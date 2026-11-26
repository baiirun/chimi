//! A value behind a pthread mutex that this crate takes itself. The lock
//! calls go through the C library, where ThreadSanitizer intercepts them, so
//! its race checks on the C side still see each holder's accesses ordered
//! even though it cannot see inside this crate. Use it for state that Rust
//! code will reach without a `runtime.c` lock held; [`crate::serialized`]
//! stays for state the C side still guards.
//!
//! The mutex is the platform default, so a holder that reenters the lock
//! deadlocks instead of aborting: `with` bodies must not call back into code
//! that takes the same lock.

use core::cell::UnsafeCell;

unsafe extern "C" {
    fn abort() -> !;
}

pub(crate) struct Locked<T> {
    mutex: UnsafeCell<libc::pthread_mutex_t>,
    value: UnsafeCell<T>,
}

// SAFETY: the value is reached only inside `with`, while the mutex is held.
unsafe impl<T: Send> Sync for Locked<T> {}

impl<T> Locked<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self { mutex: UnsafeCell::new(libc::PTHREAD_MUTEX_INITIALIZER), value: UnsafeCell::new(value) }
    }

    /// Runs `body` on the value with the mutex held, and aborts if the C
    /// library refuses the lock. `body` must not take this lock again.
    pub(crate) fn with<R>(&self, body: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: the mutex was statically initialised and is never moved
        // while locked: a `Locked` lives in a static.
        if unsafe { libc::pthread_mutex_lock(self.mutex.get()) } != 0 {
            // SAFETY: abort takes no arguments and never returns.
            unsafe { abort() }
        }
        let _unlock = Unlock(self.mutex.get());
        // SAFETY: the mutex is held, so this is the only reference.
        body(unsafe { &mut *self.value.get() })
    }
}

/// Releases the mutex when `with` returns, or unwinds in a unit test.
struct Unlock(*mut libc::pthread_mutex_t);

impl Drop for Unlock {
    fn drop(&mut self) {
        // SAFETY: `with` locked this mutex on this thread.
        unsafe { libc::pthread_mutex_unlock(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn holders_on_many_threads_see_each_others_writes() {
        static SHARED: Locked<u64> = Locked::new(0);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                thread::spawn(|| {
                    for _ in 0..10_000 {
                        SHARED.with(|value| *value += 1);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(SHARED.with(|value| *value), 80_000);
    }

    #[test]
    fn the_lock_is_free_again_after_a_body_panics() {
        static SHARED: Locked<u32> = Locked::new(0);
        let _ = thread::spawn(|| SHARED.with(|_| panic!("in the body"))).join();
        assert_eq!(SHARED.with(|value| *value), 0);
    }
}
