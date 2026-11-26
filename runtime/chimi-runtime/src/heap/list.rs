//! Reading the persistent list vector (`ListVector`): a 32-way trie of
//! full leaves plus a tail leaf of the last 1 to 32 elements. The builders
//! stay in `runtime.c` until the values move (phase 4).

use super::layout::{LIST_BITS, ListNode, ListVector, Value};

const MASK: u32 = (1 << LIST_BITS) - 1;

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
