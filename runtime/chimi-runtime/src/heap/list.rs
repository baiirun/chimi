//! The persistent list vector (`ListVector`): a 32-way trie of full leaves
//! plus a tail leaf of the last 1 to 32 elements, and the List values that
//! are windows of one (CHI-305 step 5, phase 4). Every operation copies the
//! nodes it changes and shares the rest, so a vector is never written once
//! a List holds it, except by [`slice`]'s compaction, which swaps in an
//! equal vector.

use core::ffi::c_void;
use core::ptr;

use alloc::vec::Vec;

use super::allocator::{allocate, carriers_on_threads};
use super::fragment::{allocating_into, set_kind};
use super::layout::{
    AllocationArea, AllocationHeader, AllocationKind, AreaRole, Carrier, LIST_BITS, ListNode, ListVector, Value,
    ValueKind,
};
use super::values::new_value;

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
    /// Prints "Chimi runtime list length overflow" and exits 70.
    fn chimi_runtime_list_length_overflow() -> !;
}

const WIDTH: u32 = 1 << LIST_BITS;
const MASK: u32 = WIDTH - 1;

/// A window shorter than 1/`RETAINED_FRACTION` of its vector is copied.
const RETAINED_FRACTION: u64 = 4;

const EMPTY: ListVector = ListVector { root: ptr::null_mut(), tail: ptr::null_mut(), length: 0 };

/// The index of the first element stored in the tail rather than the trie.
fn tail_offset(length: u32) -> u32 {
    if length == 0 { 0 } else { ((length - 1) >> LIST_BITS) << LIST_BITS }
}

/// The leaf holding element `index` of `vector`.
///
/// # Safety
/// `index` is below `vector.length` and `vector` is live.
unsafe fn leaf(vector: ListVector, index: u32) -> *mut ListNode {
    if index >= tail_offset(vector.length) {
        return vector.tail;
    }
    // SAFETY: the caller's contract; an inner node's slots are child nodes.
    unsafe {
        let mut node = vector.root;
        for height in (1..=(*node).height).rev() {
            let slots = (&raw mut (*node).slots).cast::<*mut ListNode>();
            node = *slots.add(((index >> (height * LIST_BITS)) & MASK) as usize);
        }
        node
    }
}

/// Element `index` of the List `list`, or null past its end.
///
/// # Safety
/// `list` is a live List value.
pub unsafe fn get(list: *const Value, index: u32) -> *mut Value {
    // SAFETY: the caller's contract; a leaf's slots are values.
    unsafe {
        let window = (*list).payload.list;
        if index >= window.length {
            return core::ptr::null_mut();
        }
        let index = index + window.start;
        let node = leaf(window.vector, index);
        *(&raw mut (*node).slots).cast::<*mut Value>().add((index & MASK) as usize)
    }
}

/// The slots of `node`, which follow it.
///
/// # Safety
/// `node` is live; the caller stays within its count.
unsafe fn slots(node: *mut ListNode) -> *mut *mut c_void {
    // SAFETY: the caller's contract.
    unsafe { (&raw mut (*node).slots).cast() }
}

/// A node of `height` with `count` null slots.
///
/// # Safety
/// The running carrier's fiber has a live heap.
unsafe fn node(height: u32, count: u32) -> *mut ListNode {
    // SAFETY: the caller's contract; the node has room for its slots.
    unsafe {
        let node = allocate(1, size_of::<ListNode>() + count as usize * size_of::<*mut c_void>()).cast::<ListNode>();
        set_kind(node.cast(), AllocationKind::LIST_NODE);
        (*node).height = height;
        (*node).count = count;
        node
    }
}

/// A copy of `source` with `count` slots, keeping as many of its own.
///
/// # Safety
/// As for [`node`]; `source` is live.
unsafe fn node_copy(source: *mut ListNode, count: u32) -> *mut ListNode {
    // SAFETY: the caller's contract.
    unsafe {
        let copy = node((*source).height, count);
        ptr::copy_nonoverlapping(slots(source), slots(copy), (*source).count.min(count) as usize);
        copy
    }
}

/// A path of single-slot nodes from `height` down to `leaf`.
///
/// # Safety
/// As for [`node`].
unsafe fn new_path(height: u32, leaf: *mut ListNode) -> *mut ListNode {
    // SAFETY: the caller's contract. The nodes are allocated top down, as
    // the C's recursion did.
    unsafe {
        if height == 0 {
            return leaf;
        }
        let top = node(height, 1);
        let mut parent = top;
        for level in (1..height).rev() {
            let child = node(level, 1);
            *slots(parent) = child.cast();
            parent = child;
        }
        *slots(parent) = leaf.cast();
        top
    }
}

/// A copy of the path in `root` to the leaf that starts at element
/// `first_index`, with `leaf` attached there.
///
/// # Safety
/// As for [`node`]; `root` is a live inner node.
unsafe fn push_leaf(root: *mut ListNode, first_index: u32, leaf: *mut ListNode) -> *mut ListNode {
    // SAFETY: the caller's contract. Each level's copy is allocated before
    // the level below it, as the C's recursion did.
    unsafe {
        let mut source = root;
        let mut top = ptr::null_mut();
        let mut parent_slot: *mut *mut c_void = ptr::null_mut();
        loop {
            let height = (*source).height;
            let slot = (first_index >> (height * LIST_BITS)) & MASK;
            let copy = node_copy(source, slot + 1);
            if parent_slot.is_null() {
                top = copy;
            } else {
                *parent_slot = copy.cast();
            }
            let place = slots(copy).add(slot as usize);
            if height == 1 {
                *place = leaf.cast();
                return top;
            }
            if slot >= (*source).count {
                *place = new_path(height - 1, leaf).cast();
                return top;
            }
            source = (*slots(source).add(slot as usize)).cast();
            parent_slot = place;
        }
    }
}

/// `vector` with `item` appended. Amortized O(1): copies at most one tail
/// and one root-to-leaf path.
///
/// # Safety
/// As for [`node`]; `vector` is live.
pub unsafe fn push(vector: ListVector, item: *mut Value) -> ListVector {
    // SAFETY: the caller's contract.
    unsafe {
        if vector.length == u32::MAX {
            chimi_runtime_list_length_overflow();
        }
        let tail_length = vector.length - tail_offset(vector.length);
        if vector.length == 0 || tail_length < WIDTH {
            let next = if vector.length == 0 { node(0, 1) } else { node_copy(vector.tail, tail_length + 1) };
            *slots(next).add(tail_length as usize) = item.cast();
            return ListVector { root: vector.root, tail: next, length: vector.length + 1 };
        }
        let first_index = vector.length - WIDTH;
        let root = if vector.root.is_null() {
            new_path(1, vector.tail)
        } else if u64::from(first_index >> LIST_BITS) == 1u64 << ((*vector.root).height * LIST_BITS) {
            let grown = node((*vector.root).height + 1, 2);
            *slots(grown) = vector.root.cast();
            *slots(grown).add(1) = new_path((*vector.root).height, vector.tail).cast();
            grown
        } else {
            push_leaf(vector.root, first_index, vector.tail)
        };
        let next = node(0, 1);
        *slots(next) = item.cast();
        ListVector { root, tail: next, length: vector.length + 1 }
    }
}

/// A copy of the path in `top` to element `index`, holding `item` there.
///
/// # Safety
/// As for [`node`]; element `index` is under `top`.
unsafe fn set_node(top: *mut ListNode, index: u32, item: *mut Value) -> *mut ListNode {
    // SAFETY: the caller's contract; each level's copy is allocated before
    // the level below it, as the C's recursion did.
    unsafe {
        let mut source = top;
        let mut result = ptr::null_mut();
        let mut parent_slot: *mut *mut c_void = ptr::null_mut();
        loop {
            let copy = node_copy(source, (*source).count);
            if parent_slot.is_null() {
                result = copy;
            } else {
                *parent_slot = copy.cast();
            }
            let height = (*source).height;
            if height == 0 {
                *slots(copy).add((index & MASK) as usize) = item.cast();
                return result;
            }
            let slot = ((index >> (height * LIST_BITS)) & MASK) as usize;
            parent_slot = slots(copy).add(slot);
            source = (*slots(source).add(slot)).cast();
        }
    }
}

/// `vector` with element `index`, which exists, replaced by `item`.
/// O(log32 n): copies the path to it.
///
/// # Safety
/// As for [`node`]; `vector` is live.
pub unsafe fn set(vector: ListVector, index: u32, item: *mut Value) -> ListVector {
    // SAFETY: the caller's contract.
    unsafe {
        if index >= tail_offset(vector.length) {
            ListVector { root: vector.root, tail: set_node(vector.tail, index, item), length: vector.length }
        } else {
            ListVector { root: set_node(vector.root, index, item), tail: vector.tail, length: vector.length }
        }
    }
}

/// A vector of the `length` values at `values`, built in O(length) without
/// intermediate versions: the tail, then the leaves, then each level of
/// inner nodes.
///
/// # Safety
/// As for [`node`]; `values` holds `length` values.
pub unsafe fn from_values(values: *const *mut Value, length: u32) -> ListVector {
    // SAFETY: the caller's contract; every copy stays within the counts.
    unsafe {
        if length == 0 {
            return EMPTY;
        }
        let tail_start = tail_offset(length);
        let tail = node(0, length - tail_start);
        ptr::copy_nonoverlapping(values.add(tail_start as usize).cast(), slots(tail), (length - tail_start) as usize);
        let mut level_count = (tail_start >> LIST_BITS) as usize;
        if level_count == 0 {
            return ListVector { root: ptr::null_mut(), tail, length };
        }
        let mut level: Vec<*mut ListNode> = Vec::new();
        if level.try_reserve_exact(level_count).is_err() {
            chimi_runtime_exhausted_memory();
        }
        for index in 0..level_count {
            let leaf = node(0, WIDTH);
            ptr::copy_nonoverlapping(values.add(index << LIST_BITS).cast(), slots(leaf), WIDTH as usize);
            level.push(leaf);
        }
        let mut height = 0;
        while level_count > 1 || height == 0 {
            height += 1;
            let parent_count = (level_count + MASK as usize) >> LIST_BITS;
            for parent in 0..parent_count {
                let first = parent << LIST_BITS;
                let count = (level_count - first).min(WIDTH as usize);
                let inner = node(height, count as u32);
                ptr::copy_nonoverlapping(level.as_ptr().add(first).cast(), slots(inner), count);
                level[parent] = inner;
            }
            level_count = parent_count;
        }
        ListVector { root: level[0], tail, length }
    }
}

/// A List of `length` elements of `vector` from `start`.
///
/// # Safety
/// The running carrier's fiber has a live heap; the window is in `vector`.
pub unsafe fn list_value(vector: ListVector, start: u32, length: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::LIST);
        (*value).payload.list.vector = vector;
        (*value).payload.list.start = start;
        (*value).payload.list.length = length;
        value
    }
}

/// `list` with `item` appended; `list` is unchanged. A window that ends
/// before its vector does writes slot `end` into a path copy instead, so the
/// vector every other window shares is untouched.
///
/// # Safety
/// As for [`list_value`]; `list` is a live List.
pub unsafe fn list_push(list: *const Value, item: *mut Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let window = (*list).payload.list;
        let end = window.start + window.length;
        let vector = if end == window.vector.length { push(window.vector, item) } else { set(window.vector, end, item) };
        list_value(vector, window.start, window.length + 1)
    }
}

/// A fresh vector of elements [from, from + length) of `list`, allocated in
/// `area`, or in the running fiber's area when `area` is null.
///
/// # Safety
/// `here` is the running carrier, whose fiber has a live heap; `list` is a
/// live List holding the range; `area` is null or live.
pub unsafe fn copy_window(here: *mut Carrier, list: *const Value, from: u32, length: u32, area: *mut AllocationArea) -> ListVector {
    // SAFETY: the caller's contract.
    unsafe {
        let mut values: Vec<*mut Value> = Vec::new();
        if values.try_reserve_exact(length as usize).is_err() {
            chimi_runtime_exhausted_memory();
        }
        for index in 0..length {
            values.push(get(list, from + index));
        }
        let target = if area.is_null() { (*here).fiber.target } else { area };
        allocating_into(here, target, || from_values(values.as_ptr(), length))
    }
}

fn retains_too_much(window: u32, vector: u32) -> bool {
    u64::from(window) * RETAINED_FRACTION < u64::from(vector)
}

/// The header before `payload`.
fn header_of(payload: *const Value) -> *mut AllocationHeader {
    payload.cast::<AllocationHeader>().wrapping_sub(1).cast_mut()
}

/// Window [from, to) of `list`, or null when the range is reversed or past
/// its end.
///
/// The window shares the vector in O(1) unless it is shorter than a quarter
/// of it, so a small window does not keep a large vector alive. Then `list`
/// itself is compacted first: it adopts a fresh vector holding exactly its
/// elements, and the window shares that. Values are immutable, so swapping
/// the representation is unobservable, and every later window of `list`
/// shares the compact vector instead of copying again; a window shorter
/// than a quarter of `list` itself is copied on its own. Draining n elements
/// one at a time copies fewer than n/3 in total (n/4 + n/16 + ...), because
/// each copy of m elements follows at least 3m dropped ones. The
/// interpreter's `PersistentList::slice` applies the same rule.
///
/// The compact vector is allocated in the area that holds `list`, so `list`
/// never points into a younger area: a survivor's new vector is itself a
/// survivor. The in-place update is safe because a heap value belongs to one
/// fiber, which does not switch here. A module constant is the exception:
/// when carriers run on threads, every carrier may read it at once, so its
/// window is copied instead.
///
/// # Safety
/// `here` is the running carrier, whose fiber has a live heap; `list` is a
/// live List the running fiber owns or a constant.
pub unsafe fn slice(here: *mut Carrier, list: *mut Value, from: u32, to: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let window = (*list).payload.list;
        if from > to || to > window.length {
            return ptr::null_mut();
        }
        if from == to {
            return list_value(EMPTY, 0, 0);
        }
        let length = to - from;
        if retains_too_much(length, window.vector.length) {
            let area = (*header_of(list)).area;
            let shared = carriers_on_threads() && (*area).role == AreaRole::STATIC;
            if retains_too_much(length, window.length) || shared {
                return list_value(copy_window(here, list, from, length, ptr::null_mut()), 0, length);
            }
            (*list).payload.list.vector = copy_window(here, list, 0, window.length, area);
            (*list).payload.list.start = 0;
        }
        let window = (*list).payload.list;
        list_value(window.vector, window.start + from, length)
    }
}
