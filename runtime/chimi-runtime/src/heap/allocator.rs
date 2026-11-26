//! The allocator's per-allocation paths (CHI-305 step 5, phase 1):
//! `allocate`, which every value, list node, text and record of a native
//! program comes from, and `release_allocation`, which frees one, together
//! with the accounting both keep (live bytes, the footprint and its peaks)
//! and the small-block cache under them. They move as one, because the cache
//! alone, called from C on every allocation and free, cost the map loop
//! 4.6% (decisions.md, 2026-11-26).
//!
//! `runtime.c` calls these with the running carrier, which it reads from its
//! thread-local, and keeps the rest for now: the young chunks and their
//! cache, the collector, and the memory report. What this needs from it
//! comes in through the `chimi_runtime_*` imports below, which is why the
//! module builds only into the runtime archives (feature `runtime-c`).
//!
//! Every count wraps, as the C it replaced did: a carrier's counts are
//! signed or unsigned deltas that another carrier's frees can carry below
//! zero, and only their sum means anything (`heap_totals_sum`).
//!
//! The carrier is passed as a raw pointer and read through it: in the
//! carrier simulation the footprint sums both carriers, the running one
//! included, so no `&mut Carrier` may be live while it does.

use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::Relaxed};

use super::layout::{
    AllocationArea, AllocationHeader, AllocationKind, AllocationScope, AreaRole, Carrier, ProfileRow,
    RuntimeHeap, SMALL_BLOCK_GRAIN, Value, YOUNG_CHUNK_CAPACITY, YOUNG_CHUNK_LARGEST, YoungChunk,
};

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
    /// Prints "Chimi runtime allocation overflow" and exits 70.
    fn chimi_runtime_allocation_overflow() -> !;
    /// An empty young chunk from the running carrier's cache or the C
    /// library; never null.
    fn chimi_runtime_young_chunk_take() -> *mut YoungChunk;
    /// Drops the handle count a freed value holds, if it is a counted handle.
    fn chimi_runtime_release_counted_value(value: *const Value);
    /// Carrier `index`, below `chimi_runtime_carrier_count`.
    fn chimi_runtime_carrier_at(index: usize) -> *mut Carrier;
    /// Whether the small-block cache keeps freed blocks: off under
    /// AddressSanitizer, so a use after free is still reported.
    static chimi_runtime_small_block_cache: bool;
    /// How the carriers run: 0 when one carrier runs every fiber, so its own
    /// counts are the process's, else [`CARRIER_THREADS`] or [`SIMULATION`];
    /// set once, before any carrier thread starts.
    static chimi_runtime_carrier_mode: u8;
    /// The number of carriers; set with the mode.
    static chimi_runtime_carrier_count: usize;
    /// The operation profile's rows, indexed by node; null unless profiling.
    static chimi_runtime_profile_rows: *mut ProfileRow;
    /// Set once a value of this execution may be a counted handle.
    static chimi_runtime_handles_counted: AtomicBool;
    /// The process-wide footprints and their peaks with several carrier
    /// threads, which the memory report reads.
    static chimi_runtime_process_footprint_bytes: AtomicI64;
    static chimi_runtime_process_peak_footprint_bytes: AtomicI64;
    static chimi_runtime_process_fragment_footprint_bytes: AtomicI64;
    static chimi_runtime_process_peak_fragment_footprint_bytes: AtomicI64;
}

/// A carrier folds its footprint change into the process-wide counters once
/// it has moved this far from what it last folded (`runtime.c`'s
/// `FOOTPRINT_FOLD_BYTES`).
const FOOTPRINT_FOLD_BYTES: i64 = 64 * 1024;

const HEADER: usize = size_of::<AllocationHeader>();

/// Several carrier threads: each folds its footprint into the process's.
const CARRIER_THREADS: u8 = 1;
/// The carrier simulation test build, whose two carriers share one thread,
/// so a peak check sums both.
const SIMULATION: u8 = 2;

fn mode() -> u8 {
    // SAFETY: written once on the main thread before any carrier thread
    // starts, which orders it before every read on those threads.
    unsafe { ptr::addr_of!(chimi_runtime_carrier_mode).read() }
}

fn cache_enabled() -> bool {
    // SAFETY: a constant of runtime.c.
    unsafe { chimi_runtime_small_block_cache }
}

fn carrier_count() -> usize {
    // SAFETY: written once on the main thread before any carrier thread
    // starts, which orders it before every read on those threads.
    unsafe { ptr::addr_of!(chimi_runtime_carrier_count).read() }
}

/// The footprint a peak check compares. With one carrier its own delta is
/// the process's; the simulation's two carriers share one thread, so it
/// compares their sum, as one carrier would. With several carrier threads
/// one carrier's delta is not the process's, and the report's peaks come
/// from [`fold`] instead.
///
/// # Safety
/// `here` is the running carrier.
#[inline]
unsafe fn footprint_now(here: *mut Carrier, mode: u8) -> i64 {
    if mode == SIMULATION {
        // SAFETY: the simulation's carriers all live on this thread.
        return unsafe { simulated_sum(|carrier| (*carrier).heap_totals.footprint_bytes + (*carrier).pending_footprint_bytes) };
    }
    // SAFETY: the caller's contract.
    unsafe { (*here).heap_totals.footprint_bytes + (*here).pending_footprint_bytes }
}

/// As [`footprint_now`], for the fragment footprint.
///
/// # Safety
/// As for [`footprint_now`].
#[inline]
unsafe fn fragment_footprint_now(here: *mut Carrier, mode: u8) -> i64 {
    if mode == SIMULATION {
        // SAFETY: as in footprint_now.
        return unsafe { simulated_sum(|carrier| (*carrier).heap_totals.fragment_footprint_bytes) };
    }
    // SAFETY: the caller's contract.
    unsafe { (*here).heap_totals.fragment_footprint_bytes }
}

/// The sum of `count` over every carrier, out of line so the paths with real
/// carriers keep their registers.
///
/// # Safety
/// Every carrier lives on this thread (the simulation).
#[cold]
#[inline(never)]
unsafe fn simulated_sum(count: impl Fn(*mut Carrier) -> i64) -> i64 {
    // SAFETY: the caller's contract.
    (0..carrier_count()).map(|index| count(unsafe { chimi_runtime_carrier_at(index) })).sum()
}

/// Raises `peak` to `value` if it is lower.
fn raise(peak: &AtomicI64, value: i64) {
    peak.fetch_max(value, Relaxed);
}

/// Out of line, so the inlined check in [`fold`] stays one compare.
#[cold]
#[inline(never)]
fn fold_now(now: i64, folded: &mut i64, process: &AtomicI64, peak: &AtomicI64) {
    let change = now - *folded;
    *folded = now;
    raise(peak, process.fetch_add(change, Relaxed) + change);
}

/// Folds this carrier's change since its last fold into a process-wide
/// footprint once it reaches [`FOOTPRINT_FOLD_BYTES`] either way, and raises
/// the peak to the new sum. Nothing with one carrier, or in the simulation,
/// whose own peaks are already the process's. One atomic add per 64 KiB of
/// change, instead of one per allocation, keeps the carriers off one cache
/// line.
#[inline]
fn fold(mode: u8, now: i64, folded: &mut i64, process: &AtomicI64, peak: &AtomicI64) {
    if mode != CARRIER_THREADS {
        return;
    }
    let change = now - *folded;
    if change < FOOTPRINT_FOLD_BYTES && change > -FOOTPRINT_FOLD_BYTES {
        return;
    }
    fold_now(now, folded, process, peak);
}

/// Folds the running carrier's footprint.
///
/// # Safety
/// `here` is the running carrier.
#[inline]
unsafe fn fold_footprint(here: *mut Carrier, mode: u8) {
    // SAFETY: the caller's contract; the folded count is this carrier's own.
    unsafe {
        let now = footprint_now(here, mode);
        fold(
            mode,
            now,
            &mut *ptr::addr_of_mut!((*here).folded_footprint_bytes),
            &chimi_runtime_process_footprint_bytes,
            &chimi_runtime_process_peak_footprint_bytes,
        );
    }
}

/// # Safety
/// `here` is the running carrier.
#[inline]
unsafe fn record_peak(here: *mut Carrier, mode: u8) {
    // SAFETY: the caller's contract.
    unsafe {
        let footprint = footprint_now(here, mode);
        if footprint > (*here).heap_totals.peak_footprint_bytes {
            (*here).heap_totals.peak_footprint_bytes = footprint;
        }
        fold(
            mode,
            footprint,
            &mut *ptr::addr_of_mut!((*here).folded_footprint_bytes),
            &chimi_runtime_process_footprint_bytes,
            &chimi_runtime_process_peak_footprint_bytes,
        );
    }
}

/// Adds `bytes` to the running carrier's footprint and records the peak.
///
/// # Safety
/// `here` is the running carrier.
#[inline]
pub unsafe fn footprint_add(here: *mut Carrier, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        (*here).heap_totals.footprint_bytes = (*here).heap_totals.footprint_bytes.wrapping_add(bytes as i64);
        record_peak(here, mode());
    }
}

/// Takes `bytes` off the running carrier's footprint. The peak is recorded
/// before the fall (the young bytes deferred in `pending_footprint_bytes`
/// peak just before it), and the fall is folded after it, so a carrier that
/// only frees still brings the process count down.
///
/// # Safety
/// `here` is the running carrier.
#[inline]
pub unsafe fn footprint_sub(here: *mut Carrier, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        let mode = mode();
        record_peak(here, mode);
        (*here).heap_totals.footprint_bytes = (*here).heap_totals.footprint_bytes.wrapping_sub(bytes as i64);
        fold_footprint(here, mode);
    }
}

/// # Safety
/// `here` is the running carrier.
unsafe fn fold_fragment_footprint(here: *mut Carrier, mode: u8) {
    // SAFETY: the caller's contract.
    unsafe {
        let now = fragment_footprint_now(here, mode);
        fold(
            mode,
            now,
            &mut *ptr::addr_of_mut!((*here).folded_fragment_footprint_bytes),
            &chimi_runtime_process_fragment_footprint_bytes,
            &chimi_runtime_process_peak_fragment_footprint_bytes,
        );
    }
}

/// Adds `bytes` to the running carrier's fragment footprint and records
/// its peak.
///
/// # Safety
/// `here` is the running carrier.
pub unsafe fn fragment_footprint_add(here: *mut Carrier, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        let mode = mode();
        (*here).heap_totals.fragment_footprint_bytes = (*here).heap_totals.fragment_footprint_bytes.wrapping_add(bytes as i64);
        let footprint = fragment_footprint_now(here, mode);
        if footprint > (*here).heap_totals.peak_fragment_footprint_bytes {
            (*here).heap_totals.peak_fragment_footprint_bytes = footprint;
        }
        fold_fragment_footprint(here, mode);
    }
}

/// Takes `bytes` off the running carrier's fragment footprint.
///
/// # Safety
/// `here` is the running carrier.
pub unsafe fn fragment_footprint_sub(here: *mut Carrier, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        (*here).heap_totals.fragment_footprint_bytes = (*here).heap_totals.fragment_footprint_bytes.wrapping_sub(bytes as i64);
        fold_fragment_footprint(here, mode());
    }
}

/// Starts the process-wide counters from carrier 0, the only carrier before
/// more are created, so the peaks keep whatever carrier 0 reached before.
///
/// # Safety
/// `first` is carrier 0, and no other carrier runs yet.
pub unsafe fn seed_process_footprint(first: *mut Carrier) {
    // SAFETY: the caller's contract.
    unsafe {
        let footprint = (*first).heap_totals.footprint_bytes + (*first).pending_footprint_bytes;
        let fragments = (*first).heap_totals.fragment_footprint_bytes;
        (*first).folded_footprint_bytes = footprint;
        (*first).folded_fragment_footprint_bytes = fragments;
        chimi_runtime_process_footprint_bytes.store(footprint, Relaxed);
        chimi_runtime_process_fragment_footprint_bytes.store(fragments, Relaxed);
        chimi_runtime_process_peak_footprint_bytes.store((*first).heap_totals.peak_footprint_bytes, Relaxed);
        chimi_runtime_process_peak_fragment_footprint_bytes
            .store((*first).heap_totals.peak_fragment_footprint_bytes, Relaxed);
    }
}

/// # Safety
/// `scope` is live.
unsafe fn scope_add_live(scope: *mut AllocationScope, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        (*scope).live_bytes = (*scope).live_bytes.wrapping_add(bytes);
        if (*scope).live_bytes > (*scope).peak_live_bytes {
            (*scope).peak_live_bytes = (*scope).live_bytes;
        }
    }
}

/// # Safety
/// `heap` is live.
unsafe fn heap_add_live(heap: *mut RuntimeHeap, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        (*heap).live_bytes = (*heap).live_bytes.wrapping_add(bytes);
        if (*heap).live_bytes > (*heap).peak_live_bytes {
            (*heap).peak_live_bytes = (*heap).live_bytes;
        }
    }
}

/// Links `header` at the front of `area`'s list and counts it there.
///
/// # Safety
/// `area` is live and `header` is on no list.
unsafe fn area_link(area: *mut AllocationArea, header: *mut AllocationHeader) {
    // SAFETY: the caller's contract.
    unsafe {
        (*header).area = area;
        (*header).next = (*area).allocations;
        (*area).allocations = header;
        (*area).bytes = (*area).bytes.wrapping_add((*header).bytes);
        (*area).count = (*area).count.wrapping_add(1);
    }
}

/// The area the running fiber allocates in: its override target, or its
/// heap's innermost region's young area, or the heap's base.
///
/// # Safety
/// `here` is the running carrier, and its fiber's heap is live.
unsafe fn current_area(here: *mut Carrier) -> *mut AllocationArea {
    // SAFETY: the caller's contract.
    unsafe {
        let fiber = &(*here).fiber;
        if !fiber.target.is_null() {
            return fiber.target;
        }
        let heap = fiber.heap;
        if (*heap).innermost.is_null() {
            ptr::addr_of_mut!((*heap).base)
        } else {
            ptr::addr_of_mut!((*(*heap).innermost).young)
        }
    }
}

/// The bytes a young chunk allocation of `bytes` takes, header included:
/// a multiple of the grain.
fn young_rounded(bytes: usize) -> usize {
    (HEADER + bytes).div_ceil(SMALL_BLOCK_GRAIN) * SMALL_BLOCK_GRAIN
}

/// A zeroed block of `rounded` bytes (a multiple of the grain, at most
/// `YOUNG_CHUNK_LARGEST` plus a header) from `scope`'s chunks.
///
/// # Safety
/// `here` is the running carrier and `scope` its fiber's innermost region.
unsafe fn young_bump(here: *mut Carrier, scope: *mut AllocationScope, rounded: usize) -> *mut AllocationHeader {
    // SAFETY: the caller's contract; a chunk is YOUNG_CHUNK_BYTES with its
    // data at the end of the record.
    unsafe {
        if ((*scope).bump_end as usize) - ((*scope).bump as usize) < rounded {
            let chunk = chimi_runtime_young_chunk_take();
            if (*scope).last_chunk.is_null() {
                (*scope).first_chunk = chunk;
            } else {
                (*(*scope).last_chunk).used = (*scope).bump;
                (*(*scope).last_chunk).next = chunk;
            }
            (*scope).last_chunk = chunk;
            let data = ptr::addr_of_mut!((*chunk).data).cast::<u8>();
            (*scope).bump = data;
            (*scope).bump_end = data.add(YOUNG_CHUNK_CAPACITY);
            // The deferred young bytes count toward the process footprint
            // here, once per chunk, so a long pass cannot hide them from its
            // peak.
            fold_footprint(here, mode());
        }
        let header = (*scope).bump.cast::<AllocationHeader>();
        (*scope).bump = (*scope).bump.add(rounded);
        header
    }
}

/// Counts one allocation of `bytes` against the node being profiled, if any.
///
/// # Safety
/// `heap` is the running fiber's heap.
#[inline]
unsafe fn profile(heap: *const RuntimeHeap, bytes: usize) {
    // SAFETY: the rows outlive every fiber, and the node index is in range.
    unsafe {
        let rows = ptr::addr_of!(chimi_runtime_profile_rows).read();
        let node = (*heap).profiled_node_plus_one;
        if !rows.is_null() && node != 0 {
            let row = rows.add(node as usize - 1);
            AtomicU64::from_ptr(ptr::addr_of_mut!((*row).allocations)).fetch_add(1, Relaxed);
            AtomicU64::from_ptr(ptr::addr_of_mut!((*row).allocated_bytes)).fetch_add(bytes as u64, Relaxed);
        }
    }
}

/// `count` zeroed items of `size` bytes in the running fiber's area: from the
/// innermost region's young chunks when it is small enough and the region
/// does not link young allocations directly, otherwise a block of its own
/// linked into the area. Its kind is `BYTES` until `allocation_kind` changes
/// it. Ends the program when the size overflows or memory runs out.
///
/// # Safety
/// `here` is the running carrier, with a live heap.
pub unsafe fn allocate(here: *mut Carrier, count: usize, size: usize) -> *mut u8 {
    let count = count.max(1);
    let bytes = match count.checked_mul(size) {
        Some(bytes) if bytes <= usize::MAX - HEADER - SMALL_BLOCK_GRAIN => bytes,
        // SAFETY: no precondition.
        _ => unsafe { chimi_runtime_allocation_overflow() },
    };
    // SAFETY: the caller's contract covers every carrier, heap, scope and
    // area read and written below.
    unsafe {
        let fiber = &(*here).fiber;
        let heap = fiber.heap;
        let innermost = if fiber.target.is_null() { (*heap).innermost } else { ptr::null_mut() };
        if !innermost.is_null() && !(*innermost).links_young && bytes <= YOUNG_CHUNK_LARGEST {
            let header = young_bump(here, innermost, young_rounded(bytes));
            (*header).bytes = bytes;
            (*header).area = ptr::addr_of_mut!((*innermost).young);
            (*header).in_chunk = true;
            (*innermost).pending_bytes = (*innermost).pending_bytes.wrapping_add(bytes);
            (*innermost).pending_count = (*innermost).pending_count.wrapping_add(1);
            (*here).pending_footprint_bytes = (*here).pending_footprint_bytes.wrapping_add((HEADER + bytes) as i64);
            profile(heap, bytes);
            return header.add(1).cast();
        }
        let area = current_area(here);
        let header = (*here).small_blocks.take(&mut (*here).heap_totals.cached_bytes, HEADER + bytes, cache_enabled())
            .cast::<AllocationHeader>();
        if header.is_null() {
            chimi_runtime_exhausted_memory();
        }
        (*header).bytes = bytes;
        area_link(area, header);
        if !(*area).heap.is_null() {
            heap_add_live((*area).heap, bytes);
        }
        let scope = (*area).scope;
        if !scope.is_null() {
            scope_add_live(scope, bytes);
            (*scope).total_allocated_bytes = (*scope).total_allocated_bytes.wrapping_add(bytes);
            (*scope).allocated_bytes_by_kind[AllocationKind::BYTES.0 as usize] = (*scope).allocated_bytes_by_kind[AllocationKind::BYTES.0 as usize].wrapping_add(bytes);
        } else if (*area).role != AreaRole::FRAGMENT {
            (*here).unscoped_allocated_bytes = (*here).unscoped_allocated_bytes.wrapping_add(bytes as u64);
        }
        profile(heap, bytes);
        if (*area).role != AreaRole::STATIC {
            (*here).heap_totals.live_allocations = (*here).heap_totals.live_allocations.wrapping_add(1);
            footprint_add(here, HEADER + bytes);
            if (*area).role == AreaRole::FRAGMENT {
                fragment_footprint_add(here, HEADER + bytes);
            }
        }
        header.add(1).cast()
    }
}

/// Frees one allocation that its area has already unlinked: drops the
/// handle count it holds, takes it off its heap's live bytes and the
/// footprint, and gives its block back to the small-block cache.
///
/// # Safety
/// `here` is the running carrier, and `header` heads a block that
/// [`allocate`] made outside the young chunks, which nothing reaches any more.
pub unsafe fn release(here: *mut Carrier, header: *mut AllocationHeader) {
    // SAFETY: the caller's contract.
    unsafe {
        if chimi_runtime_handles_counted.load(Relaxed) && (*header).kind == AllocationKind::VALUE {
            chimi_runtime_release_counted_value(header.add(1).cast());
        }
        let area = (*header).area;
        let bytes = (*header).bytes;
        if !(*area).heap.is_null() {
            (*(*area).heap).live_bytes = (*(*area).heap).live_bytes.wrapping_sub(bytes);
        }
        if (*area).role != AreaRole::STATIC {
            (*here).heap_totals.live_allocations = (*here).heap_totals.live_allocations.wrapping_sub(1);
            footprint_sub(here, HEADER + bytes);
        }
        (*here).small_blocks.give(&mut (*here).heap_totals.cached_bytes, header.cast(), HEADER + bytes, cache_enabled());
    }
}
