//! The collector (CHI-305 step 5, phase 2): one pass over a region at a
//! loop's pass boundary or at the region's end.
//!
//! A pass marks what the roots reach, starting from the state `value` and
//! the `control` the region passes by address. A minor pass marks only the
//! region's young allocations: values never change after they are built, so
//! nothing older points to a younger value. A full pass marks the
//! survivors too and then frees the ones it did not reach. Marking never
//! scans a C or fiber stack; the roots are the only way in, which is why a
//! C frame may hold a pointer into the current pass only through them.
//!
//! Then the pass moves the marked allocations out of the young chunks into
//! survivor blocks, leaving each copy's address in the old header, rewrites
//! every pointer the copies, the marked young list allocations and the roots
//! hold, and empties the chunks ([`super::young::reset_chunks`]). Marked
//! allocations of the young list and of fragments adopted during the pass
//! join the survivors where they are; the rest are freed.
//!
//! A pointer that leaves the region is not followed. If it names another
//! heap (not a static constant), the pass reports
//! [`HeapStatus::FOREIGN_POINTER`] after collecting as usual: a heap value
//! must never point into another heap or a fragment.
//!
//! Mark epochs are unique across the process, taken from [`NEXT_MARK_EPOCH`]
//! a block at a time, so an allocation that moves between heaps or carriers
//! never carries an epoch a later pass reuses.

use core::ffi::c_void;
use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

use super::allocator::{area_link, cache_enabled, release};
use super::fragment::area_free;
use super::layout::{
    AllocationArea, AllocationHeader, AllocationKind, AllocationScope, AreaRole, Callable, Carrier, Control,
    Environment, Field, HeapStatus, ListNode, MARK_EPOCH_BLOCK, MapNode, PointerStack, REGION_FULL_MIN_GROWTH, Value,
    ValueKind,
};
use super::young;

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
    /// Prints "Chimi runtime allocation graph is inconsistent" and exits 70.
    fn chimi_runtime_allocation_graph_inconsistent() -> !;
    /// Prints "Chimi runtime allocation mark epoch exhausted" and exits 70.
    fn chimi_runtime_mark_epochs_exhausted() -> !;
    /// Records the payload of an allocation the pass freed, for the
    /// collector's oracle; called only when [`chimi_runtime_heap_verify`].
    fn chimi_runtime_heap_verify_freed(payload: *const c_void);
    /// Runs the collector's oracle on `scope` right after a pass, and aborts
    /// if it fails; called only when [`chimi_runtime_heap_verify`].
    fn chimi_runtime_heap_verify_pass(
        scope: *const AllocationScope,
        value: *const Value,
        control: *const Control,
        epoch: u64,
        full: bool,
        promoted: usize,
    );
    /// Whether this is a checking build whose oracle runs after every pass.
    static chimi_runtime_heap_verify: bool;
}

/// The next block of mark epochs no carrier has taken; 0 is never an epoch,
/// so a new header's zeroed epoch is never marked.
pub static NEXT_MARK_EPOCH: AtomicU64 = AtomicU64::new(1);

const HEADER: usize = size_of::<AllocationHeader>();

fn verifying() -> bool {
    // SAFETY: a constant of runtime.c.
    unsafe { chimi_runtime_heap_verify }
}

fn header_of(payload: *mut c_void) -> *mut AllocationHeader {
    payload.cast::<AllocationHeader>().wrapping_sub(1)
}

/// Pushes `pointer` on `stack`, growing it with the C library's `realloc`
/// as `runtime.c`'s `pointer_stack_push` does, since both grow the same
/// stacks.
///
/// # Safety
/// `stack` is a live stack whose items came from the C library.
#[inline]
pub(super) unsafe fn push(stack: *mut PointerStack, pointer: *mut c_void) {
    // SAFETY: the caller's contract; the new capacity is checked.
    unsafe {
        if (*stack).length == (*stack).capacity {
            let capacity = if (*stack).capacity == 0 { 256 } else { (*stack).capacity * 2 };
            let bytes = capacity.checked_mul(size_of::<*mut c_void>()).unwrap_or_else(|| chimi_runtime_exhausted_memory());
            let items = libc::realloc((*stack).items.cast(), bytes).cast::<*mut c_void>();
            if items.is_null() {
                chimi_runtime_exhausted_memory();
            }
            (*stack).items = items;
            (*stack).capacity = capacity;
        }
        *(*stack).items.add((*stack).length) = pointer;
        (*stack).length += 1;
    }
}

/// Pops the top of `stack`, if any.
///
/// # Safety
/// `stack` is live.
#[inline]
pub(super) unsafe fn pop(stack: *mut PointerStack) -> Option<*mut c_void> {
    // SAFETY: the caller's contract.
    unsafe {
        if (*stack).length == 0 {
            return None;
        }
        (*stack).length -= 1;
        Some(*(*stack).items.add((*stack).length))
    }
}

/// A mark epoch no pass on any carrier has used.
///
/// # Safety
/// `here` is the running carrier.
pub unsafe fn take_epoch(here: *mut Carrier) -> u64 {
    // SAFETY: the caller's contract.
    unsafe {
        if (*here).next_epoch == (*here).end_epoch {
            let block = MARK_EPOCH_BLOCK as u64;
            let start = NEXT_MARK_EPOCH.fetch_add(block, Relaxed);
            if start > u64::MAX - block {
                chimi_runtime_mark_epochs_exhausted();
            }
            (*here).next_epoch = start;
            (*here).end_epoch = start + block;
        }
        let epoch = (*here).next_epoch;
        (*here).next_epoch += 1;
        epoch
    }
}

/// Marks `pointer`, an allocation of `kind` or null, and queues it for its
/// children, unless it is outside the region or this pass has marked it.
///
/// # Safety
/// `here` is collecting, and `pointer` is null or a live payload.
#[inline(always)]
unsafe fn mark(here: *mut Carrier, pointer: *mut c_void, kind: AllocationKind) {
    // SAFETY: the caller's contract.
    unsafe {
        if pointer.is_null() {
            return;
        }
        let header = header_of(pointer);
        if (*header).kind != kind {
            chimi_runtime_allocation_graph_inconsistent();
        }
        let area = (*header).area;
        let scope = (*here).collecting_scope;
        if (*area).scope != scope {
            if (*area).heap != (*scope).heap && (*area).role != AreaRole::STATIC {
                (*here).collecting_foreign = true;
            }
            return;
        }
        if (*area).role == AreaRole::SURVIVOR && !(*here).collecting_full {
            return;
        }
        if (*header).mark_epoch == (*here).collecting_epoch {
            return;
        }
        (*header).mark_epoch = (*here).collecting_epoch;
        (*scope).scanned_allocations = (*scope).scanned_allocations.wrapping_add(1);
        push(&raw mut (*here).mark_stack, pointer);
    }
}

/// Visits each pointer `payload` holds, with the kind it must name, as a
/// mutable slot so forwarding can rewrite it. Marking and forwarding share
/// it, so they follow the same pointers.
///
/// # Safety
/// `payload` is a live allocation whose header names its kind.
#[inline(always)]
unsafe fn each_child(payload: *mut c_void, mut visit: impl FnMut(*mut *mut c_void, AllocationKind)) {
    use AllocationKind as Kind;
    // SAFETY: the caller's contract; each cast reads the payload as the kind
    // its header names, and each slot is a pointer field of that payload.
    unsafe {
        let header = header_of(payload);
        macro_rules! slot {
            ($place:expr) => {
                (&raw mut $place).cast::<*mut c_void>()
            };
        }
        match (*header).kind {
            Kind::VALUE => {
                let value = payload.cast::<Value>();
                let p = &raw mut (*value).payload;
                match (*value).kind {
                    ValueKind::TEXT | ValueKind::BYTES => visit(slot!((*p).text.bytes), Kind::BYTES),
                    ValueKind::JSON_NUMBER => visit(slot!((*p).json_number), Kind::BYTES),
                    ValueKind::LIST => {
                        visit(slot!((*p).list.vector.root), Kind::LIST_NODE);
                        visit(slot!((*p).list.vector.tail), Kind::LIST_NODE);
                    }
                    ValueKind::RECORD | ValueKind::TUPLE | ValueKind::CAUSE => {
                        visit(slot!((*p).record.fields), Kind::FIELDS)
                    }
                    ValueKind::CONSTRUCTED => visit(slot!((*p).constructed.fields), Kind::FIELDS),
                    ValueKind::CALLABLE => visit(slot!((*p).callable), Kind::CALLABLE),
                    ValueKind::MAP | ValueKind::SET => visit(slot!((*p).map), Kind::MAP_NODE),
                    _ => {}
                }
            }
            Kind::LIST_NODE => {
                let node = payload.cast::<ListNode>();
                let kind = if (*node).height == 0 { Kind::VALUE } else { Kind::LIST_NODE };
                let slots = payload.byte_add(core::mem::offset_of!(ListNode, slots)).cast::<*mut c_void>();
                for index in 0..(*node).count as usize {
                    visit(slots.add(index), kind);
                }
            }
            Kind::FIELDS => {
                let fields = payload.cast::<Field>();
                for index in 0..(*header).bytes / size_of::<Field>() {
                    visit(slot!((*fields.add(index)).value), Kind::VALUE);
                }
            }
            Kind::VALUE_POINTERS => {
                let values = payload.cast::<*mut c_void>();
                for index in 0..(*header).bytes / size_of::<*mut Value>() {
                    visit(values.add(index), Kind::VALUE);
                }
            }
            Kind::ENVIRONMENT => {
                let environment = payload.cast::<Environment>();
                visit(slot!((*environment).locals), Kind::VALUE_POINTERS);
                visit(slot!((*environment).requirements), Kind::VALUE_POINTERS);
            }
            Kind::CALLABLE => {
                let callable = payload.cast::<Callable>();
                visit(slot!((*callable).environment), Kind::ENVIRONMENT);
                visit(slot!((*callable).inner), Kind::CALLABLE);
            }
            Kind::MAP_NODE => {
                let node = payload.cast::<MapNode>();
                visit(slot!((*node).key), Kind::VALUE);
                visit(slot!((*node).value), Kind::VALUE);
                visit(slot!((*node).left), Kind::MAP_NODE);
                visit(slot!((*node).right), Kind::MAP_NODE);
            }
            Kind::CONTROL => {
                let control = payload.cast::<Control>();
                visit(slot!((*control).value), Kind::VALUE);
                visit(slot!((*control).cause), Kind::VALUE);
            }
            Kind::BYTES => {}
            // A frame environment is never a value's child, and no other
            // kind exists.
            _ => chimi_runtime_allocation_graph_inconsistent(),
        }
    }
}

/// The address of `pointer`'s survivor copy, moving it out of the region's
/// young chunks first if this pass has not; anything else stays where it is.
///
/// # Safety
/// `here` is collecting after marking, and `pointer` is null or a live
/// payload.
#[inline(always)]
unsafe fn forward(here: *mut Carrier, pointer: *mut c_void) -> *mut c_void {
    // SAFETY: the caller's contract; a marked chunk allocation's copy is a
    // fresh block of its size, header included.
    unsafe {
        if pointer.is_null() {
            return pointer;
        }
        let header = header_of(pointer);
        let scope = (*here).collecting_scope;
        if !(*header).in_chunk || (*(*header).area).scope != scope {
            return pointer;
        }
        if !(*header).next.is_null() {
            return (*header).next.add(1).cast();
        }
        if (*header).mark_epoch != (*here).collecting_epoch {
            chimi_runtime_allocation_graph_inconsistent();
        }
        moved(here, scope, header)
    }
}

/// Copies the chunk allocation `header` into a survivor block and returns
/// the copy's payload.
///
/// # Safety
/// As for [`forward`], with `header` marked, in a chunk and not yet moved.
#[inline(never)]
unsafe fn moved(here: *mut Carrier, scope: *mut AllocationScope, header: *mut AllocationHeader) -> *mut c_void {
    // SAFETY: the caller's contract.
    unsafe {
        let total = HEADER + (*header).bytes;
        let copy = (*here)
            .small_blocks
            .take(&mut (*here).heap_totals.cached_bytes, total, cache_enabled())
            .cast::<AllocationHeader>();
        if copy.is_null() {
            chimi_runtime_exhausted_memory();
        }
        ptr::copy_nonoverlapping(header.cast::<u8>(), copy.cast::<u8>(), total);
        (*copy).in_chunk = false;
        area_link(&raw mut (*scope).survivors, copy);
        (*scope).promoted_bytes = (*scope).promoted_bytes.wrapping_add((*copy).bytes);
        (*header).next = copy;
        let payload = copy.add(1).cast::<c_void>();
        push(&raw mut (*here).copy_stack, payload);
        payload
    }
}

/// Rewrites the pointers `payload` holds to their survivor copies.
///
/// # Safety
/// As for [`forward`], with `payload` live.
unsafe fn forward_children(here: *mut Carrier, payload: *mut c_void) {
    // SAFETY: the caller's contract.
    unsafe { each_child(payload, |slot, _| *slot = forward(here, *slot)) }
}

/// Forwards the children of the marked allocations on `area`'s list.
///
/// # Safety
/// As for [`forward`], with `area` live.
unsafe fn forward_marked_list(here: *mut Carrier, area: *mut AllocationArea) {
    // SAFETY: the caller's contract.
    unsafe {
        let mut header = (*area).allocations;
        while !header.is_null() {
            if (*header).mark_epoch == (*here).collecting_epoch {
                forward_children(here, header.add(1).cast());
            }
            header = (*header).next;
        }
    }
}

/// Moves what the roots reach out of `scope`'s chunks and updates the roots.
///
/// # Safety
/// As for [`forward`]; `value` and `control` are null or live roots.
unsafe fn forward_survivors(
    here: *mut Carrier,
    scope: *mut AllocationScope,
    value: *mut *mut Value,
    control: *mut *mut Control,
) {
    // SAFETY: the caller's contract.
    unsafe {
        if !young::chunks_used(scope) {
            return;
        }
        if !value.is_null() {
            *value = forward(here, (*value).cast()).cast();
        }
        if !control.is_null() {
            *control = forward(here, (*control).cast()).cast();
        }
        forward_marked_list(here, &raw mut (*scope).young);
        let mut area = (*scope).adopted;
        while !area.is_null() {
            forward_marked_list(here, area);
            area = (*area).next;
        }
        while let Some(payload) = pop(&raw mut (*here).copy_stack) {
            forward_children(here, payload);
        }
    }
}

/// Frees one allocation the pass did not reach, which its list no longer
/// holds.
///
/// # Safety
/// `here` is collecting and `header` an unlinked block allocation.
unsafe fn free(here: *mut Carrier, header: *mut AllocationHeader) {
    // SAFETY: the caller's contract.
    unsafe {
        if verifying() {
            chimi_runtime_heap_verify_freed(header.add(1).cast());
        }
        release(here, header);
    }
}

/// Moves the marked allocations of a young `area` to the survivors and frees
/// the rest, leaving `area` empty.
///
/// # Safety
/// `here` is collecting `scope`, and `area` is its young area or one it
/// adopted.
unsafe fn promote_marked(here: *mut Carrier, scope: *mut AllocationScope, area: *mut AllocationArea) {
    // SAFETY: the caller's contract.
    unsafe {
        let mut header = (*area).allocations;
        while !header.is_null() {
            let next = (*header).next;
            if (*header).mark_epoch == (*here).collecting_epoch {
                area_link(&raw mut (*scope).survivors, header);
                (*scope).promoted_bytes = (*scope).promoted_bytes.wrapping_add((*header).bytes);
            } else {
                (*scope).live_bytes = (*scope).live_bytes.wrapping_sub((*header).bytes);
                free(here, header);
            }
            header = next;
        }
        (*area).allocations = ptr::null_mut();
        (*area).bytes = 0;
        (*area).count = 0;
    }
}

/// Frees the survivors that this full pass did not mark.
///
/// # Safety
/// `here` is collecting `scope` in a full pass, after marking.
unsafe fn sweep_survivors(here: *mut Carrier, scope: *mut AllocationScope) {
    // SAFETY: the caller's contract.
    unsafe {
        let area = &raw mut (*scope).survivors;
        let mut link = &raw mut (*area).allocations;
        while !(*link).is_null() {
            let header = *link;
            if (*header).mark_epoch == (*here).collecting_epoch {
                link = &raw mut (*header).next;
                continue;
            }
            *link = (*header).next;
            (*area).bytes = (*area).bytes.wrapping_sub((*header).bytes);
            (*area).count = (*area).count.wrapping_sub(1);
            (*scope).live_bytes = (*scope).live_bytes.wrapping_sub((*header).bytes);
            free(here, header);
        }
    }
}

/// One pass over `scope`, keeping what `*value` and `*control` reach, and
/// rewriting them to where it moved them. `full` marks and sweeps the
/// survivors too; `ending` gives every chunk back, the first one included.
/// Returns [`HeapStatus::FOREIGN_POINTER`], after collecting as usual, when
/// a root reaches a value in another heap or in a fragment.
///
/// # Safety
/// `here` is the running carrier, not collecting; `scope` is its fiber's
/// innermost region; `value` and `control` are null or point to null or a
/// live allocation of that kind.
pub unsafe fn sweep(
    here: *mut Carrier,
    scope: *mut AllocationScope,
    value: *mut *mut Value,
    control: *mut *mut Control,
    full: bool,
    ending: bool,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe {
        let epoch = take_epoch(here);
        (*here).collecting_epoch = epoch;
        young::flush_counts(here, scope);
        (*here).collecting_scope = scope;
        (*here).collecting_full = full;
        (*here).collecting_foreign = false;
        let root = |slot: *mut *mut c_void| if slot.is_null() { ptr::null_mut() } else { *slot };
        mark(here, root(value.cast()), AllocationKind::VALUE);
        mark(here, root(control.cast()), AllocationKind::CONTROL);
        while let Some(payload) = pop(&raw mut (*here).mark_stack) {
            each_child(payload, |slot, kind| mark(here, *slot, kind));
        }
        let foreign = (*here).collecting_foreign;
        let young_bytes = (*scope).young.bytes;
        let promoted_before = (*scope).promoted_bytes;
        forward_survivors(here, scope, value, control);
        young::reset_chunks(here, scope, !ending);
        promote_marked(here, scope, &raw mut (*scope).young);
        let promoted = (*scope).promoted_bytes.wrapping_sub(promoted_before);
        (*scope).links_young = promoted.wrapping_mul(4) > young_bytes;
        while !(*scope).adopted.is_null() {
            let area = (*scope).adopted;
            (*scope).adopted = (*area).next;
            promote_marked(here, scope, area);
            area_free(here, area);
        }
        if full {
            sweep_survivors(here, scope);
            (*scope).full_collections = (*scope).full_collections.wrapping_add(1);
        }
        (*here).collecting_scope = ptr::null_mut();
        (*scope).collections = (*scope).collections.wrapping_add(1);
        if verifying() {
            chimi_runtime_heap_verify_pass(
                scope,
                root(value.cast()).cast(),
                root(control.cast()).cast(),
                epoch,
                full,
                (*scope).promoted_bytes.wrapping_sub(promoted_before),
            );
        }
        if foreign { HeapStatus::FOREIGN_POINTER } else { HeapStatus::OK }
    }
}

/// A pass boundary: a full pass once the survivors have doubled since the
/// last full pass and grown by [`REGION_FULL_MIN_GROWTH`], else a minor one.
///
/// # Safety
/// As for [`sweep`].
pub unsafe fn collect(
    here: *mut Carrier,
    scope: *mut AllocationScope,
    value: *mut *mut Value,
    control: *mut *mut Control,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe {
        let survivors = (*scope).survivors.bytes;
        let baseline = (*scope).full_baseline;
        let full = (*scope).has_baseline
            && survivors >= baseline.wrapping_mul(2)
            && survivors.wrapping_sub(baseline) >= REGION_FULL_MIN_GROWTH;
        let status = sweep(here, scope, value, control, full, false);
        if full || (!(*scope).has_baseline && (*scope).survivors.bytes != 0) {
            (*scope).has_baseline = true;
            (*scope).full_baseline = (*scope).survivors.bytes;
        }
        status
    }
}

/// A region's end: a full pass keeps what the roots reach, and the
/// survivors join the enclosing region's young generation, or the heap's
/// base area ([`young::scope_end`]).
///
/// # Safety
/// As for [`sweep`].
pub unsafe fn end(
    here: *mut Carrier,
    scope: *mut AllocationScope,
    value: *mut *mut Value,
    control: *mut *mut Control,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe {
        let status = sweep(here, scope, value, control, true, true);
        young::scope_end(scope);
        status
    }
}
