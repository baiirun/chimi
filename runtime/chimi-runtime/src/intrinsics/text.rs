//! A growing text on the running fiber's heap, as `runtime.c`'s
//! `TextBuffer`: it starts at 64 bytes and doubles, each growth a new
//! allocation that copies the old bytes, and it keeps a NUL after its bytes.
//! The growth is the C's step for step, so memory reports do not change.

use core::ptr;

use crate::heap::allocator::allocate;

pub struct HeapText {
    data: *mut u8,
    length: usize,
    capacity: usize,
}

impl HeapText {
    pub const fn new() -> Self {
        Self { data: ptr::null_mut(), length: 0, capacity: 0 }
    }

    /// The bytes so far, NUL-terminated; null while nothing was written.
    pub fn data(&self) -> *mut u8 {
        self.data
    }

    pub fn len(&self) -> usize {
        self.length
    }

    /// Room for `extra` more bytes and the NUL.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate.
    unsafe fn reserve(&mut self, extra: usize) {
        if self.length + extra < self.capacity {
            return;
        }
        let mut capacity = if self.capacity == 0 { 64 } else { self.capacity };
        while capacity < self.length + extra + 1 {
            capacity *= 2;
        }
        // SAFETY: the caller's contract; the old bytes fit in the new block.
        unsafe {
            let data = allocate(capacity, 1);
            if !self.data.is_null() {
                ptr::copy_nonoverlapping(self.data, data, self.capacity.min(capacity));
            }
            self.data = data;
        }
        self.capacity = capacity;
    }

    /// Appends `bytes`.
    ///
    /// # Safety
    /// Runs on a fiber that may allocate.
    pub unsafe fn push_bytes(&mut self, bytes: &[u8]) {
        // SAFETY: the caller's contract; `reserve` made room for the bytes
        // and the NUL.
        unsafe {
            self.reserve(bytes.len());
            ptr::copy_nonoverlapping(bytes.as_ptr(), self.data.add(self.length), bytes.len());
            self.length += bytes.len();
            *self.data.add(self.length) = 0;
        }
    }
}
