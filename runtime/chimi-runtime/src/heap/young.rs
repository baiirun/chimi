//! The young generation's chunks and the allocation scopes they belong to
//! (CHI-305 step 5, phase 1, second PR): the per-carrier cache of empty
//! chunks, the deferred counts of a pass's chunk allocations, the reset of a
//! region's chunks at a pass boundary, and a region's start and end.
//!
//! A region (an `AllocationScope`) bumps its small allocations into 16 KiB
//! chunks ([`super::allocator::allocate`]) without counting each one; the
//! counts are added once, at the pass boundary or when a nested region
//! starts ([`flush_counts`]). At the boundary the collector moves what is
//! still reachable out of the chunks, and [`reset_chunks`] frees the rest
//! and empties the chunks for the next pass ([`super::collector`] runs the
//! pass).
//!
//! As in [`super::allocator`], every count wraps, and the carrier is passed
//! as a raw pointer.

use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

use super::allocator::{footprint_add, footprint_sub, heap_add_live, scope_add_live};
use super::layout::{
    AllocationArea, AllocationHeader, AllocationKind, AllocationScope, AreaRole, Carrier, SMALL_BLOCK_GRAIN, Value,
    YOUNG_CHUNK_BYTES, YOUNG_CHUNK_CACHE, YOUNG_CHUNK_CAPACITY, YoungChunk,
};

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
    /// Drops the handle count a freed value holds, if it is a counted handle.
    fn chimi_runtime_release_counted_value(value: *const Value);
    /// Records an emptied chunk's allocations, `data` up to `used`, for the
    /// collector's oracle; called only when [`chimi_runtime_heap_verify`].
    fn chimi_runtime_heap_verify_chunk(data: *const u8, used: *const u8);
    /// Whether this is a checking build whose oracle runs after every pass.
    static chimi_runtime_heap_verify: bool;
    /// Whether this is a checking build that frees every chunk at a pass
    /// boundary after filling it with a pattern, so a pointer kept across
    /// the boundary reads garbage, or fails as a use after free under
    /// AddressSanitizer.
    static chimi_runtime_poison_young_chunks: bool;
    /// Set once a value of this execution may be a counted handle.
    static chimi_runtime_handles_counted: AtomicBool;
}

const HEADER: usize = size_of::<AllocationHeader>();

/// The byte `poison` fills a chunk with.
const POISON: u8 = 0xdb;

fn verifying() -> bool {
    // SAFETY: a constant of runtime.c.
    unsafe { chimi_runtime_heap_verify }
}

fn poisoning() -> bool {
    // SAFETY: a constant of runtime.c.
    unsafe { chimi_runtime_poison_young_chunks }
}

/// The bytes a chunk allocation of `bytes` takes, header included: a
/// multiple of the grain.
#[inline]
pub fn rounded(bytes: usize) -> usize {
    // A mask, not `div_ceil`, which costs a branch-free but longer sequence
    // on every allocation of the reset's walk.
    const _: () = assert!(SMALL_BLOCK_GRAIN.is_power_of_two());
    (HEADER + bytes + SMALL_BLOCK_GRAIN - 1) & !(SMALL_BLOCK_GRAIN - 1)
}

/// An empty chunk, from the running carrier's cache or the C library. Cached
/// and new chunks are zero, so a bump needs no zeroing of its own.
///
/// # Safety
/// `here` is the running carrier.
pub unsafe fn chunk_take(here: *mut Carrier) -> *mut YoungChunk {
    // SAFETY: the caller's contract; a chunk is YOUNG_CHUNK_BYTES.
    unsafe {
        let mut chunk = (*here).young_chunk_cache;
        if chunk.is_null() {
            chunk = libc::calloc(1, YOUNG_CHUNK_BYTES).cast::<YoungChunk>();
            if chunk.is_null() {
                chimi_runtime_exhausted_memory();
            }
        } else {
            (*here).young_chunk_cache = (*chunk).next;
            (*here).young_chunk_cache_count = (*here).young_chunk_cache_count.wrapping_sub(1);
        }
        (*chunk).next = ptr::null_mut();
        (*chunk).used = data(chunk);
        chunk
    }
}

/// Gives back a chunk whose allocations end at its `used`: into the cache,
/// zeroed, while it has room, otherwise to the C library.
///
/// # Safety
/// `here` is the running carrier, and nothing reaches `chunk` any more.
pub unsafe fn chunk_give(here: *mut Carrier, chunk: *mut YoungChunk) {
    // SAFETY: the caller's contract.
    unsafe {
        if poisoning() {
            ptr::write_bytes(data(chunk), POISON, YOUNG_CHUNK_CAPACITY);
            libc::free(chunk.cast());
            return;
        }
        if ((*here).young_chunk_cache_count as usize) < YOUNG_CHUNK_CACHE {
            zero_used(chunk);
            (*chunk).next = (*here).young_chunk_cache;
            (*here).young_chunk_cache = chunk;
            (*here).young_chunk_cache_count = (*here).young_chunk_cache_count.wrapping_add(1);
            return;
        }
        libc::free(chunk.cast());
    }
}

/// # Safety
/// `chunk` is a live chunk.
unsafe fn data(chunk: *mut YoungChunk) -> *mut u8 {
    // SAFETY: the caller's contract; the data is the record's tail.
    unsafe { ptr::addr_of_mut!((*chunk).data).cast() }
}

/// Zeroes what `chunk`'s allocations used.
///
/// # Safety
/// `chunk` is a live chunk whose `used` is in it.
unsafe fn zero_used(chunk: *mut YoungChunk) {
    // SAFETY: the caller's contract.
    unsafe {
        let start = data(chunk);
        ptr::write_bytes(start, 0, (*chunk).used as usize - start as usize);
    }
}

/// Adds the chunk allocations made since the last call to the memory
/// counters. A pass only adds to its scope's and heap's counters until its
/// boundary, so deferring the additions to the boundary (or to a nested
/// region's start) leaves those peaks the same. The global footprint can
/// fall mid-pass, so its peak counts the deferred bytes through
/// `pending_footprint_bytes`.
///
/// # Safety
/// `here` is the running carrier and `scope` a live region of its fiber's
/// heap.
pub unsafe fn flush_counts(here: *mut Carrier, scope: *mut AllocationScope) {
    // SAFETY: the caller's contract.
    unsafe {
        let bytes = (*scope).pending_bytes;
        let count = (*scope).pending_count;
        if count == 0 {
            return;
        }
        (*scope).pending_bytes = 0;
        (*scope).pending_count = 0;
        (*scope).young.bytes = (*scope).young.bytes.wrapping_add(bytes);
        (*scope).young.count = (*scope).young.count.wrapping_add(count);
        heap_add_live((*scope).heap, bytes);
        scope_add_live(scope, bytes);
        (*scope).total_allocated_bytes = (*scope).total_allocated_bytes.wrapping_add(bytes);
        let kind = AllocationKind::BYTES.0 as usize;
        (*scope).allocated_bytes_by_kind[kind] = (*scope).allocated_bytes_by_kind[kind].wrapping_add(bytes);
        (*here).heap_totals.live_allocations = (*here).heap_totals.live_allocations.wrapping_add(count as i64);
        let footprint = count.wrapping_mul(HEADER).wrapping_add(bytes);
        (*here).pending_footprint_bytes = (*here).pending_footprint_bytes.wrapping_sub(footprint as i64);
        footprint_add(here, footprint);
    }
}

/// Whether this pass of `scope` has bumped an allocation into its chunks.
///
/// # Safety
/// `scope` is a live region.
pub unsafe fn chunks_used(scope: *const AllocationScope) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        let first = (*scope).first_chunk;
        !first.is_null() && (!(*first).next.is_null() || (*scope).bump != data(first))
    }
}

/// Releases the chunk allocations of this pass that the collector did not
/// move (a moved one's `next` names its copy), and empties the chunks for
/// the next pass: it keeps the first one when `keep_first`, and gives the
/// rest back. Requires [`flush_counts`] first.
///
/// # Safety
/// `here` is the running carrier and `scope` a live region of its fiber's
/// heap, at a pass boundary.
pub unsafe fn reset_chunks(here: *mut Carrier, scope: *mut AllocationScope, keep_first: bool) {
    // SAFETY: the caller's contract; every header up to a chunk's `used` was
    // written by a bump.
    unsafe {
        let first = (*scope).first_chunk;
        if first.is_null() || (keep_first && !chunks_used(scope)) {
            return;
        }
        (*(*scope).last_chunk).used = (*scope).bump;
        let handles = chimi_runtime_handles_counted.load(Relaxed);
        let mut dead_bytes = 0usize;
        let mut dead_count = 0usize;
        let mut chunk = first;
        while !chunk.is_null() {
            if verifying() {
                chimi_runtime_heap_verify_chunk(data(chunk), (*chunk).used);
            }
            let mut cursor = data(chunk);
            let used = (*chunk).used;
            while cursor < used {
                let header = cursor.cast::<AllocationHeader>();
                cursor = cursor.add(rounded((*header).bytes));
                if !(*header).next.is_null() {
                    continue;
                }
                dead_bytes = dead_bytes.wrapping_add((*header).bytes);
                dead_count = dead_count.wrapping_add(1);
                if handles && (*header).kind == AllocationKind::VALUE {
                    chimi_runtime_release_counted_value(header.add(1).cast());
                }
            }
            chunk = (*chunk).next;
        }
        (*scope).live_bytes = (*scope).live_bytes.wrapping_sub(dead_bytes);
        (*(*scope).heap).live_bytes = (*(*scope).heap).live_bytes.wrapping_sub(dead_bytes);
        (*here).heap_totals.live_allocations = (*here).heap_totals.live_allocations.wrapping_sub(dead_count as i64);
        footprint_sub(here, dead_count.wrapping_mul(HEADER).wrapping_add(dead_bytes));
        let mut rest = (*first).next;
        if keep_first && !poisoning() {
            zero_used(first);
            (*first).next = ptr::null_mut();
            (*first).used = data(first);
            (*scope).last_chunk = first;
            (*scope).bump = data(first);
            (*scope).bump_end = data(first).add(YOUNG_CHUNK_CAPACITY);
        } else {
            chunk_give(here, first);
            (*scope).first_chunk = ptr::null_mut();
            (*scope).last_chunk = ptr::null_mut();
            (*scope).bump = ptr::null_mut();
            (*scope).bump_end = ptr::null_mut();
        }
        while !rest.is_null() {
            let next = (*rest).next;
            chunk_give(here, rest);
            rest = next;
        }
    }
}

/// Starts `scope` as the innermost region of the running fiber's heap. The
/// enclosing region's counts are flushed first, so its peak counts what its
/// pass has allocated so far.
///
/// # Safety
/// `here` is the running carrier, whose fiber has a live heap, and `scope`
/// is writable and outlives its region.
pub unsafe fn scope_begin(here: *mut Carrier, scope: *mut AllocationScope) {
    // SAFETY: the caller's contract; an all-zero scope is valid (null
    // pointers, zero counts, the YOUNG role).
    unsafe {
        ptr::write_bytes(scope, 0, 1);
        let heap = (*here).fiber.heap;
        let parent = (*heap).innermost;
        if !parent.is_null() {
            flush_counts(here, parent);
        }
        (*scope).heap = heap;
        (*scope).parent = parent;
        (*scope).young.role = AreaRole::YOUNG;
        (*scope).young.heap = heap;
        (*scope).young.scope = scope;
        (*scope).survivors.role = AreaRole::SURVIVOR;
        (*scope).survivors.heap = heap;
        (*scope).survivors.scope = scope;
        (*heap).innermost = scope;
    }
}

/// Moves every allocation of `source` to the front of `target`'s list.
///
/// # Safety
/// Both areas are live and distinct.
pub unsafe fn area_merge(target: *mut AllocationArea, source: *mut AllocationArea) {
    // SAFETY: the caller's contract.
    unsafe {
        let mut header = (*source).allocations;
        if header.is_null() {
            return;
        }
        loop {
            (*header).area = target;
            if (*header).next.is_null() {
                break;
            }
            header = (*header).next;
        }
        (*header).next = (*target).allocations;
        (*target).allocations = (*source).allocations;
        (*target).bytes = (*target).bytes.wrapping_add((*source).bytes);
        (*target).count = (*target).count.wrapping_add((*source).count);
        (*source).allocations = ptr::null_mut();
        (*source).bytes = 0;
        (*source).count = 0;
    }
}

/// Ends `scope`, whose last pass the collector has swept: it stops being
/// the innermost region, and its survivors join the enclosing region's young
/// generation, or the heap's base area. The enclosing region takes its
/// counts, and its peak, nested on top of what the enclosing region held.
///
/// # Safety
/// `scope` is its heap's innermost region, swept, with empty chunks.
pub unsafe fn scope_end(scope: *mut AllocationScope) {
    // SAFETY: the caller's contract.
    unsafe {
        let heap = (*scope).heap;
        let parent = (*scope).parent;
        (*heap).innermost = parent;
        let target = if parent.is_null() {
            ptr::addr_of_mut!((*heap).base)
        } else {
            ptr::addr_of_mut!((*parent).young)
        };
        area_merge(target, ptr::addr_of_mut!((*scope).survivors));
        if parent.is_null() {
            return;
        }
        let nested_peak = (*parent).live_bytes.wrapping_add((*scope).peak_live_bytes);
        if nested_peak > (*parent).peak_live_bytes {
            (*parent).peak_live_bytes = nested_peak;
        }
        (*parent).live_bytes = (*parent).live_bytes.wrapping_add((*scope).live_bytes);
        (*parent).total_allocated_bytes = (*parent).total_allocated_bytes.wrapping_add((*scope).total_allocated_bytes);
        for kind in 0..AllocationKind::COUNT {
            (*parent).allocated_bytes_by_kind[kind] =
                (*parent).allocated_bytes_by_kind[kind].wrapping_add((*scope).allocated_bytes_by_kind[kind]);
        }
        (*parent).collections = (*parent).collections.wrapping_add((*scope).collections);
        (*parent).full_collections = (*parent).full_collections.wrapping_add((*scope).full_collections);
        (*parent).scanned_allocations = (*parent).scanned_allocations.wrapping_add((*scope).scanned_allocations);
        (*parent).promoted_bytes = (*parent).promoted_bytes.wrapping_add((*scope).promoted_bytes);
    }
}
