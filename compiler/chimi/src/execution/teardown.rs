//! Dropping deep values without recursing on the host stack (E18).
//!
//! A derived `Drop` frees a value's parts by recursing into them, a few host frames
//! per nesting level, so a value nested a million levels deep overflows any stack.
//! A value type routes its `Drop` through `release` instead. The first value a
//! thread releases runs a loop over a per-thread queue; every part with parts of
//! its own that is freed while the loop runs is moved onto the queue instead of
//! being freed in place. The host stack then holds one level of a value at a time,
//! and the queue, on the heap, holds the rest.
//!
//! Shared parts (`Arc`) need no special care: their own drop frees the contents
//! only for the last owner, so a part another value still holds is never queued.

use std::cell::{Cell, RefCell};
use std::thread::LocalKey;

/// The per-thread state of one value type's releases.
pub(super) struct ReleaseQueue<T> {
    /// A release loop is running on this thread.
    draining: Cell<bool>,
    /// The next release is the queued value the loop is freeing, so it frees in place.
    freeing_queued: Cell<bool>,
    pending: RefCell<Vec<T>>,
}

impl<T> ReleaseQueue<T> {
    pub(super) const fn new() -> Self {
        Self {
            draining: Cell::new(false),
            freeing_queued: Cell::new(false),
            pending: RefCell::new(Vec::new()),
        }
    }
}

/// Frees `value` without recursing into its nested parts. Call it from `Drop::drop`
/// when `value` has parts that are values of the same type, passing `take`, which
/// moves the contents out and leaves a value with no parts behind.
pub(super) fn release<T: 'static>(
    queue: &'static LocalKey<ReleaseQueue<T>>,
    value: &mut T,
    take: impl FnOnce(&mut T) -> T,
) {
    // During thread teardown the queue may already be gone; then a value frees the
    // derived way, which is correct and only bounded by the stack.
    let _ = queue.try_with(|queue| {
        if queue.freeing_queued.replace(false) {
            return;
        }
        let value = take(value);
        if queue.draining.get() {
            queue.pending.borrow_mut().push(value);
            return;
        }
        queue.draining.set(true);
        let mut next = Some(value);
        while let Some(value) = next {
            queue.freeing_queued.set(true);
            // Frees `value` itself; each part it owns alone lands on `pending`.
            drop(value);
            next = queue.pending.borrow_mut().pop();
        }
        queue.draining.set(false);
    });
}
