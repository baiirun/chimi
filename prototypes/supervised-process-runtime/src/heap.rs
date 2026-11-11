//! Rust view of the C per-owner heap in `c/heap.h`. Contracts live there.

use std::ptr::NonNull;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    Int = 1,
    Text = 2,
    Items = 3,
}

#[repr(C)]
pub union ValuePayload {
    pub integer: i64,
    pub text: *mut u8,
    pub items: *mut *mut Value,
}

#[repr(C)]
pub struct Value {
    pub kind: ValueKind,
    pub length: u32,
    pub as_: ValuePayload,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeapStatus {
    Ok = 0,
    ForeignPointer = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HeapStats {
    pub live_bytes: usize,
    pub peak_live_bytes: usize,
    pub copied_bytes: usize,
    pub scanned_values: u64,
    pub minor_collections: u64,
    pub full_collections: u64,
    pub open_regions: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChunkStats {
    pub live_chunks: usize,
    pub live_chunk_bytes: usize,
}

#[repr(C)]
pub struct RawHeap {
    _private: [u8; 0],
}

#[repr(C)]
pub struct RawFragment {
    _private: [u8; 0],
}

unsafe extern "C" {
    pub fn chimi_heap_create(owner: u64) -> *mut RawHeap;
    pub fn chimi_heap_destroy(heap: *mut RawHeap);
    pub fn chimi_heap_switch(heap: *mut RawHeap) -> *mut RawHeap;
    pub fn chimi_value_int(value: i64) -> *mut Value;
    pub fn chimi_value_text(bytes: *const u8, length: usize) -> *mut Value;
    pub fn chimi_value_items(items: *const *mut Value, length: u32) -> *mut Value;
    pub fn chimi_region_enter();
    pub fn chimi_region_pass(state: *mut *mut Value) -> HeapStatus;
    pub fn chimi_region_exit();
    pub fn chimi_fragment_copy(root: *const Value, out: *mut *mut RawFragment) -> HeapStatus;
    pub fn chimi_fragment_adopt(fragment: *mut RawFragment) -> *mut Value;
    pub fn chimi_fragment_free(fragment: *mut RawFragment);
    pub fn chimi_value_owner(value: *const Value) -> u64;
    pub fn chimi_heap_stats(heap: *const RawHeap) -> HeapStats;
    pub fn chimi_chunk_stats() -> ChunkStats;
}

/// An owned heap. Dropping it frees every chunk.
pub struct Heap(NonNull<RawHeap>);

impl Heap {
    /// Requires: `owner` is nonzero and unique among live heaps.
    pub fn new(owner: u64) -> Self {
        assert_ne!(owner, 0, "owner 0 marks detached fragments");
        let raw = unsafe { chimi_heap_create(owner) };
        Self(NonNull::new(raw).expect("chimi_heap_create aborts instead of returning null"))
    }

    /// Ensures: this heap is current until the guard drops, then the previous
    /// heap is current again.
    pub fn enter(&self) -> HeapGuard<'_> {
        let previous = unsafe { chimi_heap_switch(self.raw()) };
        HeapGuard { previous, _heap: std::marker::PhantomData }
    }

    pub fn stats(&self) -> HeapStats {
        unsafe { chimi_heap_stats(self.raw()) }
    }

    pub(crate) fn raw(&self) -> *mut RawHeap {
        self.0.as_ptr()
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        unsafe { chimi_heap_destroy(self.raw()) }
    }
}

/// Restores the previously current heap when dropped.
pub struct HeapGuard<'heap> {
    previous: *mut RawHeap,
    _heap: std::marker::PhantomData<&'heap Heap>,
}

impl Drop for HeapGuard<'_> {
    fn drop(&mut self) {
        unsafe { chimi_heap_switch(self.previous) };
    }
}

/// A detached copy in transit. Dropping an unadopted fragment frees it.
#[derive(Debug)]
pub struct Fragment(NonNull<RawFragment>);

impl Fragment {
    /// Copies `root` out of the current heap. See `chimi_fragment_copy`.
    ///
    /// # Safety
    /// `root` points to a live value, and a heap is current.
    pub unsafe fn copy(root: *const Value) -> Result<Self, HeapStatus> {
        let mut out = std::ptr::null_mut();
        match unsafe { chimi_fragment_copy(root, &mut out) } {
            HeapStatus::Ok => Ok(Self(NonNull::new(out).expect("Ok status carries a fragment"))),
            status => Err(status),
        }
    }

    /// Joins the current heap and returns the copied root. See `chimi_fragment_adopt`.
    pub fn adopt(self) -> *mut Value {
        let raw = self.0.as_ptr();
        std::mem::forget(self);
        unsafe { chimi_fragment_adopt(raw) }
    }
}

impl Drop for Fragment {
    fn drop(&mut self) {
        unsafe { chimi_fragment_free(self.0.as_ptr()) }
    }
}

/// Chunk accounting for the calling thread. See `chimi_chunk_stats`.
pub fn chunk_stats() -> ChunkStats {
    unsafe { chimi_chunk_stats() }
}
