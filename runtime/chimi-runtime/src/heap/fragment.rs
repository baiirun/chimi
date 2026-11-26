//! Fragments and owner heaps (CHI-305 step 5, phase 3, first PR): the copy
//! of a value graph made for a transfer between owners, its adoption by the
//! receiving heap, and the creation and destruction of a heap.
//!
//! A copy keeps sharing through a forwarding table, so it is linear in the
//! distinct allocations it reaches, and it rebuilds each List window as a
//! compact vector, so sending a short slice of a long list copies only the
//! slice. Static constants are shared, not copied. A copy never follows a
//! pointer out of its source (the sending heap, or the fragment it copies
//! again): it reports [`HeapStatus::FOREIGN_POINTER`] instead, because a
//! heap value must never point into another heap or a fragment.
//!
//! A fragment is a [`RuntimeFragment`] record whose area holds the copy;
//! adoption hands that area to the receiving heap's innermost region (or
//! its base area) in O(1), and the record then lives on as that area until
//! the collector or the heap's destruction frees it.

use alloc::vec::Vec;
use core::ffi::c_void;
use core::mem::size_of;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

use super::allocator::{
    allocate, cache_enabled, footprint_add, footprint_sub, fragment_footprint_add, fragment_footprint_sub,
    heap_add_live, release, scope_add_live,
};
use super::layout::{
    AllocationArea, AllocationHeader, AllocationKind, AreaRole, Callable, Carrier, Control, Environment, Field,
    FRAGMENT_ROOTS, HeapStatus, ListVector, MapNode, RuntimeFragment, RuntimeHeap, Value, ValueKind,
};

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
    /// Prints "Chimi runtime allocation graph is inconsistent" and exits 70.
    fn chimi_runtime_allocation_graph_inconsistent() -> !;
    /// Prints "Chimi runtime destroyed a heap with an open region" and exits
    /// 70.
    fn chimi_runtime_heap_destroyed_open() -> !;
    /// Adds the handle count a copied value holds, if it is a counted handle.
    fn chimi_runtime_retain_counted_value(value: *const Value);
    /// Item `index` of the List `list`, inside its window.
    fn chimi_runtime_list_get_v1(list: *const Value, index: u32) -> *mut Value;
    /// A compact vector of `values`, built in the current area.
    fn chimi_runtime_vector_from_values_v1(values: *const *mut Value, length: u32, out: *mut ListVector);
}

/// The next owner id a new heap takes; the main heap is owner 1.
pub static NEXT_HEAP_OWNER: AtomicU64 = AtomicU64::new(2);

const INLINE_SLOTS: usize = 16;

fn header_of(payload: *const c_void) -> *mut AllocationHeader {
    payload.cast::<AllocationHeader>().cast_mut().wrapping_sub(1)
}

/// The kind of the allocation `payload`.
///
/// # Safety
/// `payload` is a live allocation.
unsafe fn kind_of(payload: *const c_void) -> AllocationKind {
    // SAFETY: the caller's contract.
    unsafe { (*header_of(payload)).kind }
}

/// Where a copy reads from: every allocation of one heap, or of one area.
#[derive(Clone, Copy)]
enum Source {
    Heap(*mut RuntimeHeap),
    Area(*const AllocationArea),
}

/// The forwarding table from each source allocation to its copy: open
/// addressing, at most half full. Small messages fit the inline slots, so
/// copying one allocates nothing besides the copy.
struct Table {
    inline_keys: [*mut c_void; INLINE_SLOTS],
    inline_copies: [*mut c_void; INLINE_SLOTS],
    /// Once the inline slots are outgrown: the keys, then the copies.
    spilled: Vec<*mut c_void>,
    capacity: usize,
    count: usize,
}

fn slot_of(pointer: *const c_void, capacity: usize) -> usize {
    let hash = (pointer as usize as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (hash >> 20) as usize & (capacity - 1)
}

impl Table {
    fn new() -> Self {
        Self {
            inline_keys: [ptr::null_mut(); INLINE_SLOTS],
            inline_copies: [ptr::null_mut(); INLINE_SLOTS],
            spilled: Vec::new(),
            capacity: INLINE_SLOTS,
            count: 0,
        }
    }

    fn slots(&self) -> (&[*mut c_void], &[*mut c_void]) {
        if self.spilled.is_empty() {
            (&self.inline_keys, &self.inline_copies)
        } else {
            self.spilled.split_at(self.capacity)
        }
    }

    fn slots_mut(&mut self) -> (&mut [*mut c_void], &mut [*mut c_void]) {
        if self.spilled.is_empty() {
            (&mut self.inline_keys, &mut self.inline_copies)
        } else {
            self.spilled.split_at_mut(self.capacity)
        }
    }

    fn insert(&mut self, key: *mut c_void, value: *mut c_void) {
        if 2 * (self.count + 1) > self.capacity {
            let capacity = self.capacity * 2;
            let mut spilled: Vec<*mut c_void> = Vec::new();
            if spilled.try_reserve_exact(2 * capacity).is_err() {
                // SAFETY: a function of runtime.c that does not return.
                unsafe { chimi_runtime_exhausted_memory() }
            }
            spilled.resize(2 * capacity, ptr::null_mut());
            let (keys, copies) = spilled.split_at_mut(capacity);
            let (old_keys, old_copies) = self.slots();
            for (old, copy) in old_keys.iter().zip(old_copies) {
                if old.is_null() {
                    continue;
                }
                let mut slot = slot_of(*old, capacity);
                while !keys[slot].is_null() {
                    slot = (slot + 1) & (capacity - 1);
                }
                keys[slot] = *old;
                copies[slot] = *copy;
            }
            self.spilled = spilled;
            self.capacity = capacity;
        }
        let capacity = self.capacity;
        let (keys, copies) = self.slots_mut();
        let mut slot = slot_of(key, capacity);
        while !keys[slot].is_null() {
            slot = (slot + 1) & (capacity - 1);
        }
        keys[slot] = key;
        copies[slot] = value;
        self.count += 1;
    }

    fn find(&self, key: *const c_void) -> *mut c_void {
        let (keys, copies) = self.slots();
        let mut slot = slot_of(key, self.capacity);
        while !keys[slot].is_null() {
            if keys[slot].cast_const() == key {
                return copies[slot];
            }
            slot = (slot + 1) & (self.capacity - 1);
        }
        ptr::null_mut()
    }
}

/// One copy of a graph: its source, its target, and its forwarding table.
struct Copy {
    source: Source,
    target: *mut AllocationArea,
    table: Table,
    status: HeapStatus,
    /// The owner of the first foreign allocation reached (0 for a
    /// fragment's).
    foreign_owner: u64,
}

impl Copy {
    fn new(source: Source, target: *mut AllocationArea) -> Self {
        Self { source, target, table: Table::new(), status: HeapStatus::OK, foreign_owner: 0 }
    }

    /// Whether the copy may read `area`.
    ///
    /// # Safety
    /// `area` is live.
    unsafe fn owns(&self, area: *const AllocationArea) -> bool {
        match self.source {
            Source::Area(source) => area == source,
            // SAFETY: the caller's contract.
            Source::Heap(heap) => unsafe { (*area).heap == heap },
        }
    }

    /// Records that the graph leaves the source at `area`.
    ///
    /// # Safety
    /// `area` is live.
    unsafe fn foreign(&mut self, area: *const AllocationArea) {
        // SAFETY: the caller's contract.
        unsafe {
            if self.status == HeapStatus::OK {
                self.foreign_owner = if (*area).heap.is_null() { 0 } else { (*(*area).heap).owner };
            }
        }
        self.status = HeapStatus::FOREIGN_POINTER;
    }
}

/// Changes the kind `payload`'s header names, moving its bytes between its
/// region's per-kind counts.
///
/// # Safety
/// `payload` is a live allocation.
unsafe fn set_kind(payload: *mut c_void, kind: AllocationKind) {
    // SAFETY: the caller's contract.
    unsafe {
        let header = header_of(payload);
        let scope = (*(*header).area).scope;
        if !scope.is_null() && (*header).kind != kind {
            let counts = &raw mut (*scope).allocated_bytes_by_kind;
            let bytes = (*header).bytes;
            (*counts)[(*header).kind.0 as usize] = (*counts)[(*header).kind.0 as usize].wrapping_sub(bytes);
            (*counts)[kind.0 as usize] = (*counts)[kind.0 as usize].wrapping_add(bytes);
        }
        (*header).kind = kind;
    }
}

/// Runs `body` with the running fiber allocating into `target`.
///
/// # Safety
/// `here` is the running carrier and `target` a live area.
unsafe fn allocating_into<T>(here: *mut Carrier, target: *mut AllocationArea, body: impl FnOnce() -> T) -> T {
    // SAFETY: the caller's contract.
    unsafe {
        let previous = (*here).fiber.target;
        (*here).fiber.target = target;
        let result = body();
        (*here).fiber.target = previous;
        result
    }
}

/// The copy of `pointer`, an allocation of `kind` or null, made on first
/// sight and queued for [`scan`]; null when it is outside the source.
///
/// # Safety
/// `here` is the running carrier and `pointer` null or a live payload.
unsafe fn forward(here: *mut Carrier, copy: &mut Copy, pointer: *mut c_void, kind: AllocationKind) -> *mut c_void {
    // SAFETY: the caller's contract; the duplicate is a fresh allocation of
    // the source's size.
    unsafe {
        if pointer.is_null() {
            return pointer;
        }
        let header = header_of(pointer);
        if (*header).kind != kind {
            chimi_runtime_allocation_graph_inconsistent();
        }
        let area = (*header).area;
        if (*area).role == AreaRole::STATIC {
            return pointer;
        }
        if !copy.owns(area) {
            copy.foreign(area);
            return ptr::null_mut();
        }
        let existing = copy.table.find(pointer);
        if !existing.is_null() {
            return existing;
        }
        let bytes = (*header).bytes;
        let duplicate = allocating_into(here, copy.target, || allocate(bytes, 1)).cast::<c_void>();
        set_kind(duplicate, kind);
        ptr::copy_nonoverlapping(pointer.cast::<u8>(), duplicate.cast::<u8>(), bytes);
        if (*here).copy_counting {
            (*here).copied_bytes = (*here).copied_bytes.wrapping_add(bytes as u64);
            if kind == AllocationKind::VALUE {
                (*here).copied_values = (*here).copied_values.wrapping_add(1);
            }
        }
        if kind == AllocationKind::VALUE {
            chimi_runtime_retain_counted_value(duplicate.cast());
        }
        copy.table.insert(pointer, duplicate);
        super::collector::push(&raw mut (*here).copy_pending, duplicate);
        duplicate
    }
}

/// Rebuilds the duplicate List `list`'s window as a compact vector of copied
/// items, or empties it when its nodes are outside the source.
///
/// # Safety
/// As for [`forward`], with `list` a fresh duplicate List.
unsafe fn copy_list_window(here: *mut Carrier, copy: &mut Copy, list: *mut Value) {
    // SAFETY: the caller's contract.
    unsafe {
        let payload = &raw mut (*list).payload.list;
        let source = (*payload).vector;
        let leaves = |node: *mut super::layout::ListNode| -> *const AllocationArea {
            if node.is_null() { ptr::null() } else { (*header_of(node.cast())).area }
        };
        let (root, tail) = (leaves(source.root), leaves(source.tail));
        let outside = |area: *const AllocationArea, copy: &Copy| {
            !area.is_null() && (*area).role != AreaRole::STATIC && !copy.owns(area)
        };
        if outside(root, copy) || outside(tail, copy) {
            copy.foreign(if !root.is_null() && !copy.owns(root) { root } else { tail });
            (*payload).vector = ListVector { root: ptr::null_mut(), tail: ptr::null_mut(), length: 0 };
            (*payload).start = 0;
            (*payload).length = 0;
            return;
        }
        let length = (*payload).length;
        let mut values = Vec::new();
        if values.try_reserve_exact(length as usize + 1).is_err() {
            chimi_runtime_exhausted_memory();
        }
        for index in 0..length {
            let item = chimi_runtime_list_get_v1(list, index);
            values.push(forward(here, copy, item.cast(), AllocationKind::VALUE).cast::<Value>());
        }
        allocating_into(here, copy.target, || {
            chimi_runtime_vector_from_values_v1(values.as_ptr(), length, &raw mut (*payload).vector)
        });
        (*payload).start = 0;
    }
}

/// Redirects the pointers of a fresh duplicate from its source's graph to
/// copies.
///
/// # Safety
/// As for [`forward`], with `duplicate` a fresh duplicate.
unsafe fn scan(here: *mut Carrier, copy: &mut Copy, duplicate: *mut c_void) {
    use AllocationKind as Kind;
    // SAFETY: the caller's contract; each cast reads the duplicate as the
    // kind its header names.
    unsafe {
        macro_rules! redirect {
            ($place:expr, $kind:expr) => {
                $place = forward(here, copy, $place as *mut c_void, $kind).cast()
            };
        }
        let header = header_of(duplicate);
        match (*header).kind {
            Kind::VALUE => {
                let value = duplicate.cast::<Value>();
                let p = &raw mut (*value).payload;
                match (*value).kind {
                    ValueKind::TEXT | ValueKind::BYTES => redirect!((*p).text.bytes, Kind::BYTES),
                    ValueKind::JSON_NUMBER => redirect!((*p).json_number, Kind::BYTES),
                    ValueKind::LIST => copy_list_window(here, copy, value),
                    ValueKind::RECORD | ValueKind::CAUSE | ValueKind::TUPLE => {
                        redirect!((*p).record.fields, Kind::FIELDS)
                    }
                    ValueKind::CONSTRUCTED => redirect!((*p).constructed.fields, Kind::FIELDS),
                    ValueKind::CALLABLE => redirect!((*p).callable, Kind::CALLABLE),
                    ValueKind::MAP | ValueKind::SET => redirect!((*p).map, Kind::MAP_NODE),
                    _ => {}
                }
            }
            Kind::FIELDS => {
                let fields = duplicate.cast::<Field>();
                for index in 0..(*header).bytes / size_of::<Field>() {
                    redirect!((*fields.add(index)).value, Kind::VALUE);
                }
            }
            Kind::VALUE_POINTERS => {
                let values = duplicate.cast::<*mut Value>();
                for index in 0..(*header).bytes / size_of::<*mut Value>() {
                    redirect!(*values.add(index), Kind::VALUE);
                }
            }
            Kind::ENVIRONMENT => {
                let environment = duplicate.cast::<Environment>();
                redirect!((*environment).locals, Kind::VALUE_POINTERS);
                redirect!((*environment).requirements, Kind::VALUE_POINTERS);
            }
            Kind::CALLABLE => {
                let callable = duplicate.cast::<Callable>();
                redirect!((*callable).environment, Kind::ENVIRONMENT);
                redirect!((*callable).inner, Kind::CALLABLE);
            }
            Kind::MAP_NODE => {
                let node = duplicate.cast::<MapNode>();
                redirect!((*node).key, Kind::VALUE);
                redirect!((*node).value, Kind::VALUE);
                redirect!((*node).left, Kind::MAP_NODE);
                redirect!((*node).right, Kind::MAP_NODE);
            }
            Kind::CONTROL => {
                let control = duplicate.cast::<Control>();
                redirect!((*control).value, Kind::VALUE);
                redirect!((*control).cause, Kind::VALUE);
            }
            Kind::BYTES => {}
            // List nodes are rebuilt with their List, never copied alone; a
            // frame environment is never a value's child.
            _ => chimi_runtime_allocation_graph_inconsistent(),
        }
    }
}

/// Copies the graphs under `roots` (with their `kinds`) into the copy's
/// target, writing the copied roots to `out`.
///
/// # Safety
/// `here` is the running carrier; `roots`, `kinds` and `out` hold `count`
/// entries; each root is null or a live payload of its kind.
unsafe fn copy_graph(
    here: *mut Carrier,
    copy: &mut Copy,
    roots: *const *mut c_void,
    kinds: *const AllocationKind,
    count: usize,
    out: *mut *mut c_void,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe {
        for index in 0..count {
            *out.add(index) = forward(here, copy, *roots.add(index), *kinds.add(index));
        }
        while let Some(duplicate) = super::collector::pop(&raw mut (*here).copy_pending) {
            scan(here, copy, duplicate);
        }
        copy.status
    }
}

/// The footprint a fragment accounts: its record and its allocations with
/// their headers.
///
/// # Safety
/// `fragment` is live.
unsafe fn footprint(fragment: *const RuntimeFragment) -> usize {
    // SAFETY: the caller's contract.
    unsafe {
        size_of::<RuntimeFragment>()
            + (*fragment).area.bytes
            + (*fragment).area.count * size_of::<AllocationHeader>()
    }
}

/// A new, empty fragment record holding `count` roots.
///
/// # Safety
/// `here` is the running carrier.
unsafe fn fragment_new(here: *mut Carrier, count: u32) -> *mut RuntimeFragment {
    // SAFETY: the caller's contract; small blocks come zeroed.
    unsafe {
        let bytes = size_of::<RuntimeFragment>();
        let fragment = (*here)
            .small_blocks
            .take(&mut (*here).heap_totals.cached_bytes, bytes, cache_enabled())
            .cast::<RuntimeFragment>();
        if fragment.is_null() {
            chimi_runtime_exhausted_memory();
        }
        (*fragment).area.role = AreaRole::FRAGMENT;
        (*fragment).root_count = count;
        (*here).heap_totals.live_fragments = (*here).heap_totals.live_fragments.wrapping_add(1);
        footprint_add(here, bytes);
        fragment_footprint_add(here, bytes);
        fragment
    }
}

/// Frees every allocation of `area`, leaving it empty. O(allocations).
///
/// # Safety
/// `here` is the running carrier, and nothing reaches `area`'s allocations.
pub unsafe fn area_release_all(here: *mut Carrier, area: *mut AllocationArea) {
    // SAFETY: the caller's contract.
    unsafe {
        while !(*area).allocations.is_null() {
            let header = (*area).allocations;
            (*area).allocations = (*header).next;
            release(here, header);
        }
        (*area).bytes = 0;
        (*area).count = 0;
    }
}

/// Frees an emptied adopted area, which is the record of the fragment it
/// came from.
///
/// # Safety
/// `here` is the running carrier and `area` an empty fragment area.
pub unsafe fn area_free(here: *mut Carrier, area: *mut AllocationArea) {
    // SAFETY: the caller's contract; the area is a RuntimeFragment's first
    // member, allocated as a small block of the record's size.
    unsafe {
        let bytes = size_of::<RuntimeFragment>();
        footprint_sub(here, bytes);
        (*here).small_blocks.give(&mut (*here).heap_totals.cached_bytes, area.cast(), bytes, cache_enabled());
    }
}

/// The kinds of `fragment`'s roots.
///
/// # Safety
/// `fragment` is live.
unsafe fn root_kinds(fragment: *const RuntimeFragment) -> [AllocationKind; FRAGMENT_ROOTS] {
    let mut kinds = [AllocationKind::BYTES; FRAGMENT_ROOTS];
    // SAFETY: the caller's contract; a fragment's roots are live.
    unsafe {
        for (index, kind) in kinds.iter_mut().enumerate().take((*fragment).root_count as usize) {
            *kind = kind_of((*fragment).roots[index]);
        }
    }
    kinds
}

/// The transfer boundary: a detached fragment holding one copy of the
/// graphs under `roots`, written to `out`. Every value under them belongs
/// to the running fiber's heap, or is static; otherwise it returns
/// [`HeapStatus::FOREIGN_POINTER`], records the foreign owner on the
/// carrier, and writes no fragment.
///
/// # Safety
/// `here` is the running carrier; `roots` and `kinds` hold `count` entries,
/// at most [`FRAGMENT_ROOTS`].
pub unsafe fn copy(
    here: *mut Carrier,
    roots: *const *mut c_void,
    kinds: *const AllocationKind,
    count: u32,
    out: *mut *mut RuntimeFragment,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe {
        if count as usize > FRAGMENT_ROOTS {
            libc::abort();
        }
        let fragment = fragment_new(here, count);
        let mut copy = Copy::new(Source::Heap((*here).fiber.heap), &raw mut (*fragment).area);
        let roots_out = (&raw mut (*fragment).roots).cast::<*mut c_void>();
        let status = copy_graph(here, &mut copy, roots, kinds, count as usize, roots_out);
        if status != HeapStatus::OK {
            (*here).fragment_copy_foreign_owner = copy.foreign_owner;
            fragment_footprint_sub(here, footprint(fragment));
            area_release_all(here, &raw mut (*fragment).area);
            (*here).heap_totals.live_fragments = (*here).heap_totals.live_fragments.wrapping_sub(1);
            area_free(here, &raw mut (*fragment).area);
            *out = ptr::null_mut();
            return status;
        }
        *out = fragment;
        HeapStatus::OK
    }
}

/// Hands `fragment`'s allocations to the running fiber's heap, its
/// innermost region or else its base area, in O(1), and writes its roots to
/// `roots`. Consumes the fragment.
///
/// # Safety
/// `here` is the running carrier; `fragment` is live and unadopted; `roots`
/// holds its root count.
pub unsafe fn adopt(here: *mut Carrier, fragment: *mut RuntimeFragment, roots: *mut *mut c_void) {
    // SAFETY: the caller's contract.
    unsafe {
        for index in 0..(*fragment).root_count as usize {
            *roots.add(index) = (*fragment).roots[index];
        }
        fragment_footprint_sub(here, footprint(fragment));
        (*here).heap_totals.live_fragments = (*here).heap_totals.live_fragments.wrapping_sub(1);
        let area = &raw mut (*fragment).area;
        let heap = (*here).fiber.heap;
        let scope = (*heap).innermost;
        (*area).heap = heap;
        heap_add_live(heap, (*area).bytes);
        if scope.is_null() {
            (*area).role = AreaRole::BASE;
            (*area).next = (*heap).adopted;
            (*heap).adopted = area;
            (*here).unscoped_allocated_bytes = (*here).unscoped_allocated_bytes.wrapping_add((*area).bytes as u64);
        } else {
            (*area).role = AreaRole::YOUNG;
            (*area).scope = scope;
            (*area).next = (*scope).adopted;
            (*scope).adopted = area;
            scope_add_live(scope, (*area).bytes);
        }
    }
}

/// Copies `fragment`'s graph into the running fiber's current area,
/// keeping sharing, and leaves the fragment intact so it can be copied
/// again (Task.result). O(fragment).
///
/// # Safety
/// `here` is the running carrier; `fragment` is live; `roots` holds its root
/// count.
pub unsafe fn copy_into_current(here: *mut Carrier, fragment: *const RuntimeFragment, roots: *mut *mut c_void) {
    // SAFETY: the caller's contract.
    unsafe {
        let kinds = root_kinds(fragment);
        let target = current_area(here);
        let mut copy = Copy::new(Source::Area(&raw const (*fragment).area), target);
        let sources = (&raw const (*fragment).roots).cast::<*mut c_void>();
        let status = copy_graph(here, &mut copy, sources, kinds.as_ptr(), (*fragment).root_count as usize, roots);
        if status != HeapStatus::OK {
            chimi_runtime_allocation_graph_inconsistent();
        }
    }
}

/// A second detached copy of the unadopted fragment `source`, for another
/// broadcast or watch subscription. O(fragment).
///
/// # Safety
/// `here` is the running carrier and `source` live and unadopted.
pub unsafe fn duplicate(here: *mut Carrier, source: *const RuntimeFragment) -> *mut RuntimeFragment {
    // SAFETY: the caller's contract.
    unsafe {
        let fragment = fragment_new(here, (*source).root_count);
        let kinds = root_kinds(source);
        let mut copy = Copy::new(Source::Area(&raw const (*source).area), &raw mut (*fragment).area);
        let sources = (&raw const (*source).roots).cast::<*mut c_void>();
        let roots_out = (&raw mut (*fragment).roots).cast::<*mut c_void>();
        let status = copy_graph(here, &mut copy, sources, kinds.as_ptr(), (*source).root_count as usize, roots_out);
        if status != HeapStatus::OK {
            chimi_runtime_allocation_graph_inconsistent();
        }
        fragment
    }
}

/// Discards an unadopted fragment, for example a message handed back to
/// its sender.
///
/// # Safety
/// `here` is the running carrier and `fragment` live, unadopted and
/// unreachable.
pub unsafe fn free(here: *mut Carrier, fragment: *mut RuntimeFragment) {
    // SAFETY: the caller's contract.
    unsafe {
        fragment_footprint_sub(here, footprint(fragment));
        (*here).heap_totals.live_fragments = (*here).heap_totals.live_fragments.wrapping_sub(1);
        area_release_all(here, &raw mut (*fragment).area);
        area_free(here, &raw mut (*fragment).area);
    }
}

/// The area the running fiber allocates in.
///
/// # Safety
/// `here` is the running carrier.
unsafe fn current_area(here: *mut Carrier) -> *mut AllocationArea {
    // SAFETY: the caller's contract.
    unsafe {
        if !(*here).fiber.target.is_null() {
            return (*here).fiber.target;
        }
        let heap = (*here).fiber.heap;
        if (*heap).innermost.is_null() { &raw mut (*heap).base } else { &raw mut (*(*heap).innermost).young }
    }
}

/// A new, empty heap with a fresh owner id.
///
/// # Safety
/// `here` is the running carrier.
pub unsafe fn heap_create(here: *mut Carrier) -> *mut RuntimeHeap {
    // SAFETY: the caller's contract; calloc zeroes the record.
    unsafe {
        let heap = libc::calloc(1, size_of::<RuntimeHeap>()).cast::<RuntimeHeap>();
        if heap.is_null() {
            chimi_runtime_exhausted_memory();
        }
        (*heap).owner = NEXT_HEAP_OWNER.fetch_add(1, Relaxed);
        (*heap).base.role = AreaRole::BASE;
        (*heap).base.heap = heap;
        (*here).heap_totals.live_heaps = (*here).heap_totals.live_heaps.wrapping_add(1);
        heap
    }
}

/// Frees every allocation `heap` owns, adopted fragments included, without
/// touching any other heap, and then the heap. No region of it may be open.
///
/// # Safety
/// `here` is the running carrier; `heap` came from [`heap_create`] and
/// nothing reaches it.
pub unsafe fn heap_destroy(here: *mut Carrier, heap: *mut RuntimeHeap) {
    // SAFETY: the caller's contract.
    unsafe {
        if !(*heap).innermost.is_null() {
            chimi_runtime_heap_destroyed_open();
        }
        area_release_all(here, &raw mut (*heap).base);
        while !(*heap).adopted.is_null() {
            let area = (*heap).adopted;
            (*heap).adopted = (*area).next;
            area_release_all(here, area);
            area_free(here, area);
        }
        libc::free(heap.cast());
        (*here).heap_totals.live_heaps = (*here).heap_totals.live_heaps.wrapping_sub(1);
    }
}
